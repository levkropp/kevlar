// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    arch::{self, USER_STACK_TOP},
    ctypes::*,
    debug::{self, DebugEvent, DebugFilter},
    fs::{
        devfs::SERIAL_TTY,
        inode::FileLike,
        mount::RootFs,
        opened_file::{Fd, OpenFlags, OpenOptions, OpenedFile, OpenedFileTable, PathComponent},
        path::Path,
    },
    mm::vm::{Vm, VmAreaType},
    prelude::*,
    process::{
        cmdline::Cmdline,
        current_process,
        elf::{Elf, ProgramHeader},
        init_stack::{estimate_user_init_stack_size, init_user_stack, Auxv},
        process_group::{PgId, ProcessGroup},
        signal::{SigAction, SigSet, Signal, SignalDelivery, SignalMask, SIGCHLD, SIGCONT, SIGKILL},
        switch, UserVAddr, JOIN_WAIT_QUEUE, SCHEDULER, SchedulerPolicy,
    },
    random::read_secure_random,
    result::Errno,
    INITIAL_ROOT_FS,
};
use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use atomic_refcell::{AtomicRef, AtomicRefCell};
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use core::sync::atomic::AtomicUsize;
use crossbeam::atomic::AtomicCell;
use goblin::elf64::program_header::PT_LOAD;
use kevlar_platform::{
    arch::{PtRegs, PAGE_SIZE},
    page_allocator::{alloc_pages, AllocPageFlags},
    spinlock::{SpinLock, SpinLockGuard, SpinLockGuardNoIrq},
};
use kevlar_utils::alignment::align_up;

type ProcessTable = BTreeMap<PId, Arc<Process>>;

/// The process table. All processes are registered in with its process Id.
pub(super) static PROCESSES: SpinLock<ProcessTable> = SpinLock::new(BTreeMap::new());
pub static EXITED_PROCESSES: SpinLock<Vec<Arc<Process>>> = SpinLock::new(Vec::new());

static FORK_TOTAL: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
pub struct Stats {
    pub fork_total: usize,
}

pub fn process_count() -> usize {
    PROCESSES.lock().len()
}

pub fn list_pids() -> Vec<PId> {
    PROCESSES.lock().keys().cloned().collect()
}

pub fn read_process_stats() -> Stats {
    Stats {
        fork_total: FORK_TOTAL.load(Ordering::SeqCst),
    }
}

/// Returns an unused PID. Note that this function does not reserve the PID:
/// keep the process table locked until you insert the process into the table!
pub(super) fn alloc_pid(table: &mut ProcessTable) -> Result<PId> {
    static NEXT_PID: AtomicI32 = AtomicI32::new(2);

    let last_pid = NEXT_PID.load(Ordering::SeqCst);
    loop {
        // Note: `fetch_add` may wrap around.
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        if pid <= 1 {
            continue;
        }

        if !table.contains_key(&PId::new(pid)) {
            return Ok(PId::new(pid));
        }

        if pid == last_pid {
            return Err(Errno::EAGAIN.into());
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PId(i32);

impl PId {
    pub const fn new(pid: i32) -> PId {
        PId(pid)
    }

    pub const fn as_i32(self) -> i32 {
        self.0
    }
}

/// Process states.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ProcessState {
    /// The process is runnable.
    Runnable,
    /// The process is sleeping. It can be resumed by signals.
    BlockedSignalable,
    /// The process has been stopped by a signal (SIGSTOP/SIGTSTP/SIGTTIN/SIGTTOU).
    Stopped(Signal),
    /// The process has exited.
    ExitedWith(c_int),
}

/// The process control block.
pub struct Process {
    arch: arch::Process,
    is_idle: bool,
    process_group: AtomicRefCell<Weak<SpinLock<ProcessGroup>>>,
    pid: PId,
    /// Thread group ID. For single-threaded processes and group leaders this
    /// equals `pid`. All threads in the same thread group share the same TGID.
    tgid: PId,
    state: AtomicCell<ProcessState>,
    parent: Weak<Process>,
    cmdline: AtomicRefCell<Cmdline>,
    children: SpinLock<Vec<Arc<Process>>>,
    vm: AtomicRefCell<Option<Arc<SpinLock<Vm>>>>,
    opened_files: Arc<SpinLock<OpenedFileTable>>,
    root_fs: Arc<SpinLock<RootFs>>,
    signals: Arc<SpinLock<SignalDelivery>>,
    /// Lock-free mirror of `signals.pending`.  Avoids taking the spinlock on
    /// every syscall exit when no signals are pending (the common case).
    signal_pending: AtomicU32,
    signaled_frame: AtomicCell<Option<PtRegs>>,
    sigset: AtomicU64,
    umask: AtomicCell<u32>,
    // UID/GID tracking (Phase 5).
    uid: AtomicU32,
    euid: AtomicU32,
    gid: AtomicU32,
    egid: AtomicU32,
    /// Nice value (-20 to +19). Used by getpriority/setpriority.
    nice: AtomicI32,
    /// Whether this process is a child subreaper (PR_SET_CHILD_SUBREAPER).
    is_child_subreaper: AtomicBool,
    /// Process name set via PR_SET_NAME (max 16 bytes including NUL).
    comm: SpinLock<Option<Vec<u8>>>,
    /// Address to write 0 to (and then wake the futex) when this thread exits.
    /// Set by `set_tid_address(2)`. Used by pthread_join via futex.
    clear_child_tid: AtomicUsize,
    /// Monotonic tick count at process creation (for /proc/[pid]/stat field 22).
    start_ticks: u64,
    /// Accumulated user-mode ticks (incremented by timer IRQ).
    utime: AtomicU64,
    /// Accumulated kernel-mode ticks (incremented per syscall).
    stime: AtomicU64,
    /// cgroup v2 membership (None only for idle threads created before cgroups::init).
    cgroup: atomic_refcell::AtomicRefCell<Option<Arc<crate::cgroups::CgroupNode>>>,
    /// Namespace set (UTS, PID, mount).
    namespaces: AtomicRefCell<Option<crate::namespace::NamespaceSet>>,
    /// Namespace-local PID (equals global PID in root PID namespace).
    ns_pid: AtomicI32,
}

impl Process {
    /// Creates a per-CPU idle thread.
    ///
    /// An idle thread is a special type of kernel threads which is executed
    /// only if there're no other runnable processes.
    pub fn new_idle_thread() -> Result<Arc<Process>> {
        let process_group = ProcessGroup::new(PgId::new(0));
        let proc = Arc::new(Process {
            is_idle: true,
            process_group: AtomicRefCell::new(Arc::downgrade(&process_group)),
            arch: arch::Process::new_idle_thread(),
            state: AtomicCell::new(ProcessState::Runnable),
            parent: Weak::new(),
            cmdline: AtomicRefCell::new(Cmdline::new()),
            children: SpinLock::new(Vec::new()),
            vm: AtomicRefCell::new(None),
            pid: PId::new(0),
            tgid: PId::new(0),
            root_fs: INITIAL_ROOT_FS.clone(),
            opened_files: Arc::new(SpinLock::new(OpenedFileTable::new())),
            signals: Arc::new(SpinLock::new(SignalDelivery::new())),
            signal_pending: AtomicU32::new(0),
            signaled_frame: AtomicCell::new(None),
            sigset: AtomicU64::new(0),
            umask: AtomicCell::new(0o022),
            uid: AtomicU32::new(0),
            euid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            egid: AtomicU32::new(0),
            nice: AtomicI32::new(0),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(None),
            clear_child_tid: AtomicUsize::new(0),
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(0),
        });

        process_group.lock().add(Arc::downgrade(&proc));
        Ok(proc)
    }

    /// Creates the initial process (PID=1).
    pub fn new_init_process(
        root_fs: Arc<SpinLock<RootFs>>,
        executable_path: Arc<PathComponent>,
        console: Arc<PathComponent>,
        argv: &[&[u8]],
    ) -> Result<()> {
        assert!(console.inode.is_file());

        let mut opened_files = OpenedFileTable::new();
        // Open stdin.
        opened_files.open_with_fixed_fd(
            Fd::new(0),
            Arc::new(OpenedFile::new(
                console.clone(),
                OpenFlags::O_RDONLY.into(),
                0,
            )),
            OpenOptions::empty(),
        )?;
        // Open stdout.
        opened_files.open_with_fixed_fd(
            Fd::new(1),
            Arc::new(OpenedFile::new(
                console.clone(),
                OpenFlags::O_WRONLY.into(),
                0,
            )),
            OpenOptions::empty(),
        )?;
        // Open stderr.
        opened_files.open_with_fixed_fd(
            Fd::new(2),
            Arc::new(OpenedFile::new(console, OpenFlags::O_WRONLY.into(), 0)),
            OpenOptions::empty(),
        )?;

        let entry = setup_userspace(executable_path, argv, &[], &root_fs)?;
        let pid = PId::new(1);
        let process_group = ProcessGroup::new(PgId::new(1));
        let process = Arc::new(Process {
            is_idle: false,
            process_group: AtomicRefCell::new(Arc::downgrade(&process_group)),
            pid,
            tgid: pid,
            parent: Weak::new(),
            children: SpinLock::new(Vec::new()),
            state: AtomicCell::new(ProcessState::Runnable),
            cmdline: AtomicRefCell::new(Cmdline::from_argv(argv)),
            arch: arch::Process::new_user_thread(entry.ip, entry.user_sp),
            vm: AtomicRefCell::new(Some(Arc::new(SpinLock::new(entry.vm)))),
            opened_files: Arc::new(SpinLock::new(opened_files)),
            root_fs,
            signals: Arc::new(SpinLock::new(SignalDelivery::new())),
            signal_pending: AtomicU32::new(0),
            signaled_frame: AtomicCell::new(None),
            sigset: AtomicU64::new(0),
            umask: AtomicCell::new(0o022),
            uid: AtomicU32::new(0),
            euid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            egid: AtomicU32::new(0),
            nice: AtomicI32::new(0),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(None),
            clear_child_tid: AtomicUsize::new(0),
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(pid.as_i32()),
        });

        process_group.lock().add(Arc::downgrade(&process));
        PROCESSES.lock().insert(pid, process);
        SCHEDULER.lock().enqueue(pid);

        SERIAL_TTY.set_foreground_process_group(Arc::downgrade(&process_group));
        Ok(())
    }

    /// Returns the process with the given process ID.
    pub fn find_by_pid(pid: PId) -> Option<Arc<Process>> {
        PROCESSES.lock().get(&pid).cloned()
    }

    /// Returns true if the process is a idle kernel thread.
    pub fn is_idle(&self) -> bool {
        self.is_idle
    }

    /// The process ID.
    pub fn pid(&self) -> PId {
        self.pid
    }

    /// The thread ID.
    pub fn tid(&self) -> PId {
        // In a single-threaded process, the thread ID is equal to the process ID (PID).
        // https://man7.org/linux/man-pages/man2/gettid.2.html
        self.pid
    }

    /// The thread group ID. Threads in the same group share a TGID.
    /// `getpid()` returns this; `gettid()` returns `pid`.
    pub fn tgid(&self) -> PId {
        self.tgid
    }

    /// Monotonic tick count when this process was created.
    pub fn start_ticks(&self) -> u64 {
        self.start_ticks
    }

    /// Accumulated user-mode ticks.
    pub fn utime(&self) -> u64 {
        self.utime.load(Ordering::Relaxed)
    }

    /// Accumulated kernel-mode ticks.
    pub fn stime(&self) -> u64 {
        self.stime.load(Ordering::Relaxed)
    }

    /// Increment user-mode tick counter (called from timer IRQ).
    pub fn tick_utime(&self) {
        self.utime.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment kernel-mode tick counter (called on syscall entry).
    pub fn tick_stime(&self) {
        self.stime.fetch_add(1, Ordering::Relaxed);
    }

    /// Count threads in this thread group (processes sharing the same TGID).
    pub fn count_threads(&self) -> usize {
        let tgid = self.tgid;
        PROCESSES.lock().values().filter(|p| p.tgid == tgid).count()
    }

    /// Total virtual memory size in bytes (sum of all VMA lengths).
    pub fn vm_size_bytes(&self) -> usize {
        if let Some(ref vm_arc) = *self.vm() {
            let vm = vm_arc.lock();
            vm.vm_areas().iter()
                .map(|vma| vma.end().value() - vma.start().value())
                .sum()
        } else {
            0
        }
    }

    /// Returns the process's cgroup node (root cgroup if not set).
    pub fn cgroup(&self) -> Arc<crate::cgroups::CgroupNode> {
        self.cgroup.borrow().clone().unwrap_or_else(|| crate::cgroups::CGROUP_ROOT.clone())
    }

    /// Sets the process's cgroup node.
    pub fn set_cgroup(&self, cg: Arc<crate::cgroups::CgroupNode>) {
        *self.cgroup.borrow_mut() = Some(cg);
    }

    /// Returns the process's namespace set.
    pub fn namespaces(&self) -> crate::namespace::NamespaceSet {
        self.namespaces.borrow().clone()
            .unwrap_or_else(|| crate::namespace::root_namespace_set())
    }

    /// Returns just the UTS namespace Arc (avoids cloning the full NamespaceSet).
    pub fn uts_namespace(&self) -> alloc::sync::Arc<crate::namespace::UtsNamespace> {
        self.namespaces.borrow().as_ref()
            .map(|ns| ns.uts.clone())
            .unwrap_or_else(|| crate::namespace::ROOT_UTS.clone())
    }

    /// Sets the process's namespace set.
    pub fn set_namespaces(&self, ns: crate::namespace::NamespaceSet) {
        *self.namespaces.borrow_mut() = Some(ns);
    }

    /// Namespace-local PID (for getpid in non-root PID namespaces).
    pub fn ns_pid(&self) -> PId {
        PId::new(self.ns_pid.load(Ordering::Relaxed))
    }

    /// Set namespace-local PID (called after clone with CLONE_NEWPID).
    pub fn set_ns_pid(&self, pid: PId) {
        self.ns_pid.store(pid.as_i32(), Ordering::Relaxed);
    }

    /// Sets the `clear_child_tid` address (CLONE_CHILD_CLEARTID / set_tid_address).
    pub fn set_clear_child_tid(&self, addr: usize) {
        self.clear_child_tid.store(addr, Ordering::Relaxed);
    }

    /// The arch-specific information.
    pub fn arch(&self) -> &arch::Process {
        &self.arch
    }

    /// The process parent.
    fn parent(&self) -> Option<Arc<Process>> {
        self.parent.upgrade().as_ref().cloned()
    }

    /// The ID of process being parent of this process.
    pub fn ppid(&self) -> PId {
        if let Some(parent) = self.parent() {
            parent.pid()
        } else {
            PId::new(0)
        }
    }

    pub fn cmdline(&self) -> AtomicRef<'_, Cmdline> {
        self.cmdline.borrow()
    }

    // ── UID/GID accessors ────────────────────────────────────────────
    pub fn uid(&self) -> u32 { self.uid.load(Ordering::Relaxed) }
    pub fn euid(&self) -> u32 { self.euid.load(Ordering::Relaxed) }
    pub fn gid(&self) -> u32 { self.gid.load(Ordering::Relaxed) }
    pub fn egid(&self) -> u32 { self.egid.load(Ordering::Relaxed) }
    pub fn set_uid(&self, uid: u32) { self.uid.store(uid, Ordering::Relaxed); }
    pub fn set_euid(&self, euid: u32) { self.euid.store(euid, Ordering::Relaxed); }
    pub fn set_gid(&self, gid: u32) { self.gid.store(gid, Ordering::Relaxed); }
    pub fn set_egid(&self, egid: u32) { self.egid.store(egid, Ordering::Relaxed); }
    pub fn nice(&self) -> i32 { self.nice.load(Ordering::Relaxed) }
    pub fn set_nice(&self, n: i32) { self.nice.store(n, Ordering::Relaxed); }

    // ── Subreaper ────────────────────────────────────────────────────
    pub fn is_child_subreaper(&self) -> bool {
        self.is_child_subreaper.load(Ordering::Relaxed)
    }
    pub fn set_child_subreaper(&self, val: bool) {
        self.is_child_subreaper.store(val, Ordering::Relaxed);
    }

    // ── PR_SET_NAME / PR_GET_NAME ────────────────────────────────────
    pub fn set_comm(&self, name: &[u8]) {
        let mut comm = self.comm.lock_no_irq();
        let len = core::cmp::min(name.len(), 15); // 16 bytes max incl. NUL
        *comm = Some(name[..len].to_vec());
    }

    pub fn get_comm(&self) -> Vec<u8> {
        if let Some(ref name) = *self.comm.lock_no_irq() {
            name.clone()
        } else {
            // Fall back to argv0.
            let cmdline = self.cmdline();
            cmdline.argv0().as_bytes().to_vec()
        }
    }

    /// Its child processes.
    pub fn children(&self) -> SpinLockGuard<'_, Vec<Arc<Process>>> {
        self.children.lock()
    }

    /// The process's path resolution info.
    pub fn root_fs(&self) -> &Arc<SpinLock<RootFs>> {
        &self.root_fs
    }

    /// The opened files table (full lock with interrupt disable).
    pub fn opened_files(&self) -> &Arc<SpinLock<OpenedFileTable>> {
        &self.opened_files
    }

    /// The opened files table, locked without cli/sti.
    ///
    /// Safe because the fd table is never accessed from interrupt context.
    pub fn opened_files_no_irq(&self) -> SpinLockGuardNoIrq<'_, OpenedFileTable> {
        self.opened_files.lock_no_irq()
    }

    /// The virtual memory space. It's `None` if the process is a kernel thread.
    pub fn vm(&self) -> AtomicRef<'_, Option<Arc<SpinLock<Vm>>>> {
        self.vm.borrow()
    }

    /// Signals.
    pub fn signals(&self) -> &SpinLock<SignalDelivery> {
        &self.signals
    }

    /// Lock-free read of the pending signal bitmask.
    pub fn signal_pending_bits(&self) -> u32 {
        self.signal_pending.load(Ordering::Relaxed)
    }

    /// Update the pending signal atomic mirror (call after modifying
    /// SignalDelivery.pending while holding the signals lock).
    pub fn sync_signal_pending(&self, bits: u32) {
        self.signal_pending.store(bits, Ordering::Relaxed);
    }

    /// Loads the current signal mask (lock-free).
    #[inline(always)]
    pub fn sigset_load(&self) -> SigSet {
        SigSet::from_raw(self.sigset.load(Ordering::Relaxed))
    }

    /// Stores a new signal mask (lock-free).
    #[inline(always)]
    pub fn sigset_store(&self, set: SigSet) {
        self.sigset.store(set.bits(), Ordering::Relaxed);
    }

    /// Gets the current umask.
    #[allow(dead_code)]
    pub fn umask(&self) -> u32 {
        self.umask.load()
    }

    /// Sets the umask and returns the old value.
    pub fn set_umask(&self, new_umask: u32) -> u32 {
        self.umask.swap(new_umask & 0o777)
    }

    /// Changes the process group.
    pub fn set_process_group(&self, pg: Weak<SpinLock<ProcessGroup>>) {
        *self.process_group.borrow_mut() = pg;
    }

    /// The current process group.
    pub fn process_group(&self) -> Arc<SpinLock<ProcessGroup>> {
        self.process_group.borrow().upgrade().unwrap()
    }

    /// Returns true if the process belongs to the process group `pg`.
    pub fn belongs_to_process_group(&self, pg: &Weak<SpinLock<ProcessGroup>>) -> bool {
        Weak::ptr_eq(&self.process_group.borrow(), pg)
    }

    /// The current process state.
    pub fn state(&self) -> ProcessState {
        self.state.load()
    }

    /// Updates the process state.
    pub fn set_state(&self, new_state: ProcessState) {
        let scheduler = SCHEDULER.lock();
        self.state.store(new_state);
        match new_state {
            ProcessState::Runnable => {}
            ProcessState::BlockedSignalable
            | ProcessState::Stopped(_)
            | ProcessState::ExitedWith(_) => {
                scheduler.remove(self.pid);
            }
        }
    }

    /// Resumes a process.
    pub fn resume(&self) {
        let old_state = self.state.swap(ProcessState::Runnable);

        // A thread may be set to ExitedWith by exit_group() while it is still
        // in a wait queue (e.g. futex, poll, JOIN_WAIT_QUEUE).  When wake_all()
        // calls resume() on such a thread, we must not enqueue it — doing so
        // would schedule an exiting thread, corrupting kernel state.
        // Undo the state swap and bail out; the thread is no longer schedulable.
        if matches!(old_state, ProcessState::ExitedWith(_)) {
            self.state.store(old_state);
            return;
        }

        if old_state == ProcessState::Runnable {
            return;
        }

        // Spinwait until the process's context (kernel stack RSP/SP) has been
        // fully saved by do_switch_thread.  Without this, another CPU could
        // restore a stale RSP and run this task concurrently with the CPU that
        // is still executing do_switch_thread for it — corrupting the stack.
        //
        // Safety: We skip the spinwait when interrupts are disabled (IRQ context)
        // to avoid a rare deadlock: if the timer fires on the same CPU that is
        // mid-switch for this very thread, and the timer calls resume() on it,
        // the spinwait would block the IRQ handler forever (the assembly that
        // sets context_saved=true is waiting for the IRQ to return via IRET).
        // In that case we accept a theoretical stale-RSP race; it is mitigated
        // by the preempt_count guard that prevents the timer from calling
        // process::switch() while a switch is already in progress.
        if kevlar_platform::arch::interrupts_enabled() {
            while !self.arch.context_saved.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        }

        SCHEDULER.lock().enqueue(self.pid);
    }

    /// Stops the current process with the given signal (SIGSTOP/SIGTSTP/SIGTTIN/SIGTTOU).
    /// The process is removed from the run queue and the parent is notified via SIGCHLD.
    pub fn stop(signal: Signal) {
        let current = current_process();
        current.set_state(ProcessState::Stopped(signal));

        // Wake parent so it can collect stopped status via wait4(WUNTRACED).
        if let Some(parent) = current.parent.upgrade() {
            parent.send_signal(SIGCHLD);
        }
        JOIN_WAIT_QUEUE.wake_all();
        switch();
    }

    /// Continues a stopped process (SIGCONT handling).
    pub fn continue_process(&self) {
        if let ProcessState::Stopped(_) = self.state.load() {
            self.state.store(ProcessState::Runnable);
            SCHEDULER.lock().enqueue(self.pid);

            // Wake parent so it can collect continued status via wait4(WCONTINUED).
            if let Some(parent) = self.parent.upgrade() {
                parent.send_signal(SIGCHLD);
            }
            JOIN_WAIT_QUEUE.wake_all();
        }
    }

    /// Searches the opened file table by the file descriptor.
    ///
    /// Uses `lock_no_irq` — the fd table is never accessed from interrupt
    /// context, so we skip the cli/sti overhead.
    pub fn get_opened_file_by_fd(&self, fd: Fd) -> Result<Arc<OpenedFile>> {
        // Fast path: if the fd table is unshared (strong_count == 1, meaning
        // no threads share it), skip the spinlock and access the Vec directly.
        // This is safe because only the owning process modifies its fd table,
        // and we ARE the owning process (current_process() == self).
        //
        // This avoids the pushf/cli/cmpxchg/popf sequence per syscall that
        // dominates pipe read/write overhead.
        #[cfg(not(feature = "profile-fortress"))]
        if Arc::strong_count(&self.opened_files) == 1 {
            // SAFETY: strong_count == 1 guarantees no concurrent access.
            #[allow(unsafe_code)]
            let table = unsafe { self.opened_files.get_unchecked() };
            return Ok(table.get(fd)?.clone());
        }

        Ok(self.opened_files.lock_no_irq().get(fd)?.clone())
    }

    /// Terminates the **current** process.
    pub fn exit(status: c_int) -> ! {
        let current = current_process();
        if current.pid == PId::new(1) {
            // Dump syscall profile before halting (if profiling was enabled).
            if debug::profiler::is_enabled() {
                debug::profiler::dump_syscall_profile(
                    crate::syscalls::syscall_name_by_number,
                );
            }
            // Dump PID 1 syscall trace for debugging.
            warn!("PID 1 exiting with status {}", status);
            crate::syscalls::dump_pid1_trace();
            info!("init exited with status {}, halting system", status);
            kevlar_platform::arch::halt();
        }

        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessExit {
            pid: current.pid().as_i32(),
            status,
            by_signal: false,
        });

        current.set_state(ProcessState::ExitedWith(status));

        // Reparent children to the nearest subreaper ancestor, or init (PID 1).
        {
            let orphans: Vec<Arc<Process>> = current.children.lock().drain(..).collect();
            if !orphans.is_empty() {
                let new_parent = find_subreaper_or_init(&current);
                for child in orphans {
                    new_parent.children.lock().push(child);
                }
                // Wake wait() in case the new parent is waiting.
                JOIN_WAIT_QUEUE.wake_all();
            }
        }

        // Threads created with CLONE_VM|CLONE_THREAD have tgid != pid.
        // For thread exits: skip SIGCHLD and skip close_all (siblings share
        // the same opened_files Arc and must keep their fds open).
        let is_thread = current.tgid != current.pid;

        if is_thread {
            // Keep a reference in EXITED_PROCESSES so the Arc (and its kernel
            // stacks) stays alive through the upcoming switch().  Without this,
            // PROCESSES.lock().remove() below drops the process table's ref,
            // leaving count=1 (only CURRENT).  switch() then does:
            //   arc_leak_one_ref(&prev)  → count=1 (CURRENT)
            //   CURRENT.set(next)        → drops CURRENT → count=0 → FREED
            //   switch_thread(prev.arch, ...)  → use-after-free!
            // gc_exited_processes() frees these only from the idle thread,
            // well after switch_task has finished using prev.arch().
            EXITED_PROCESSES.lock().push(current.clone());
        }

        if !is_thread {
            if let Some(parent) = current.parent.upgrade() {
                if parent.signals().lock().nocldwait() {
                    // Parent explicitly set SIGCHLD to SIG_IGN (or SA_NOCLDWAIT):
                    // auto-reap the child without creating a zombie.
                    parent.children().retain(|p| p.pid() != current.pid);
                    EXITED_PROCESSES.lock().push(current.clone());
                } else {
                    // Normal case: keep zombie for wait(), notify parent.
                    parent.send_signal(SIGCHLD);
                }
            }

            // Close opened files here instead of in Drop::drop because `proc`
            // is not dropped until it's joined by the parent process. Drop them
            // to make pipes closed.
            current.opened_files.lock().close_all();
        }

        PROCESSES.lock().remove(&current.pid);

        // CLONE_CHILD_CLEARTID: write 0 to tid address and wake futex waiters
        // so that pthread_join() (which sleeps via futex(FUTEX_WAIT)) is woken.
        let ctid_addr = current.clear_child_tid.load(Ordering::Relaxed);
        if ctid_addr != 0 {
            if let Ok(uaddr) = crate::process::UserVAddr::new_nonnull(ctid_addr) {
                let _ = uaddr.write::<i32>(&0);
            }
            crate::syscalls::futex::futex_wake_addr(ctid_addr, 1);
        }

        JOIN_WAIT_QUEUE.wake_all();
        switch();
        unreachable!();
    }

    /// Terminates the **current** thread and other threads belonging to the same thread group.
    pub fn exit_group(status: c_int) -> ! {
        let current = current_process();
        let tgid = current.tgid;
        // Send SIGKILL to all other threads in the same thread group.
        let siblings: Vec<Arc<Process>> = {
            let table = PROCESSES.lock();
            table.values()
                .filter(|p| p.tgid == tgid && p.pid != current.pid)
                .cloned()
                .collect()
        };
        for sibling in siblings {
            sibling.set_state(ProcessState::ExitedWith(status));
            // Keep a reference in EXITED_PROCESSES so the Arc (and its kernel
            // stacks) stays alive until gc_exited_processes() runs from the
            // idle thread — even if the sibling is currently running on
            // another CPU and switch() does arc_leak_one_ref on it.
            EXITED_PROCESSES.lock().push(sibling.clone());
            // Remove from scheduler run queues FIRST so that pick_next() stops
            // returning this PID.  Only then remove from PROCESSES, so that
            // any in-flight switch() that already picked this PID can still
            // find the Arc (it will be skipped via the None-check in switch()).
            SCHEDULER.lock().remove(sibling.pid);
            PROCESSES.lock().remove(&sibling.pid);
        }
        Process::exit(status)
    }

    /// Terminates the **current** process by a signal.
    pub fn exit_by_signal(signal: Signal) -> ! {
        let pid = current_process().pid().as_i32();
        warn!("PID {} killed by signal {}", pid, signal);
        if pid == 1 {
            crate::syscalls::dump_pid1_trace();
        }
        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessExit {
            pid,
            status: 128 + signal,
            by_signal: true,
        });
        Process::exit(128 + signal);
    }

    /// Sends a signal.
    pub fn send_signal(&self, signal: Signal) {
        // SIGCONT always continues a stopped process, even if SIGCONT is blocked.
        if signal == SIGCONT {
            self.continue_process();
        }

        let mut sigs = self.signals.lock();
        let action = sigs.get_action(signal);

        // Signals with Ignore disposition (default SIGCHLD, SIGURG, SIGWINCH)
        // should NOT be queued or interrupt sleep — matching POSIX/Linux behavior.
        // If the user installs a handler (SigAction::Handler), we do queue it.
        if matches!(action, SigAction::Ignore) {
            return;
        }

        sigs.signal(signal);
        drop(sigs);

        self.signal_pending.fetch_or(1 << (signal - 1), Ordering::Release);
        self.resume();

        // Wake poll/epoll waiters so signalfd can detect the new signal.
        crate::poll::POLL_WAIT_QUEUE.wake_all();
    }

    /// Returns `true` if there's a deliverable (pending AND unblocked) signal.
    pub fn has_pending_signals(&self) -> bool {
        let pending = self.signal_pending.load(Ordering::Relaxed);
        let blocked = self.sigset_load().bits() as u32;
        (pending & !blocked) != 0
    }

    /// Sets signal mask (lock-free via atomic u64).
    #[inline(always)]
    pub fn set_signal_mask(
        &self,
        how: SignalMask,
        set: Option<UserVAddr>,
        oldset: Option<UserVAddr>,
        _length: usize,
    ) -> Result<()> {
        let mut current = self.sigset_load();

        if let Some(old) = oldset {
            old.write_bytes(&current.to_bytes())?;
        }

        if let Some(new) = set {
            let new_bytes = new.read::<[u8; 8]>()?;
            let new_set = SigSet::from_bytes(&new_bytes);
            match how {
                SignalMask::Block => current |= new_set,
                SignalMask::Unblock => current &= !new_set,
                SignalMask::Set => current = new_set,
            }
            self.sigset_store(current);
        }

        Ok(())
    }

    /// Tries to delivering a pending signal to the current process.
    ///
    /// If there's a pending signal, it may modify `frame` (e.g. user return
    /// address and stack pointer) to call the registered user's signal handler.
    pub fn try_delivering_signal(frame: &mut PtRegs) -> Result<()> {
        let current = current_process();
        // Fast path: skip the spinlock when no signals are pending.
        if current.signal_pending.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }
        let popped = {
            let mut sigs = current.signals.lock();
            let sigset = current.sigset_load();
            let result = sigs.pop_pending_unblocked(sigset);
            // Sync the atomic mirror with the actual pending state.
            current.signal_pending.store(sigs.pending_bits(), Ordering::Relaxed);
            result
        };
        if let Some((signal, sigaction)) = popped {
                let pid = current.pid().as_i32();
                let sig_name = debug::signal_name(signal);

                match sigaction {
                    SigAction::Ignore => {
                        debug::emit(DebugFilter::SIGNAL, &DebugEvent::Signal {
                            pid,
                            signal,
                            signal_name: sig_name,
                            action: "ignore",
                            handler_addr: None,
                            ip: 0,
                        });
                    }
                    SigAction::Terminate => {
                        debug::emit(DebugFilter::SIGNAL, &DebugEvent::Signal {
                            pid,
                            signal,
                            signal_name: sig_name,
                            action: "terminate",
                            handler_addr: None,
                            ip: 0,
                        });
                        trace!("terminating {:?} by {:?}", current.pid, signal,);
                        Process::exit(1 /* FIXME: */);
                    }
                    SigAction::Stop => {
                        debug::emit(DebugFilter::SIGNAL, &DebugEvent::Signal {
                            pid,
                            signal,
                            signal_name: sig_name,
                            action: "stop",
                            handler_addr: None,
                            ip: 0,
                        });
                        trace!("stopping {:?} by signal {:?}", current.pid, signal);
                        Process::stop(signal);
                    }
                    SigAction::Continue => {
                        debug::emit(DebugFilter::SIGNAL, &DebugEvent::Signal {
                            pid,
                            signal,
                            signal_name: sig_name,
                            action: "continue",
                            handler_addr: None,
                            ip: 0,
                        });
                        trace!("SIGCONT delivered to {:?} (already running)", current.pid);
                    }
                    SigAction::Handler { handler, restorer } => {
                        #[cfg(target_arch = "x86_64")]
                        let rsp_before = frame.rsp as usize;
                        #[cfg(target_arch = "aarch64")]
                        let rsp_before = frame.sp as usize;
                        debug::emit(DebugFilter::SIGNAL, &DebugEvent::Signal {
                            pid,
                            signal,
                            signal_name: sig_name,
                            action: "handler",
                            handler_addr: Some(handler.value()),
                            ip: 0,
                        });
                        trace!(
                            "delivering signal {} to pid={} via handler at {:#x}",
                            signal, pid, handler.value()
                        );
                        current.signaled_frame.store(Some(*frame));

                        // Set usercopy context for fault attribution.
                        debug::usercopy::set_context("signal_stack_setup");
                        let result = current.arch.setup_signal_stack(frame, signal, handler, restorer);
                        debug::usercopy::clear_context();

                        // Emit detailed signal stack write trace.
                        if debug::is_enabled(DebugFilter::USERCOPY) || debug::is_enabled(DebugFilter::SIGNAL) {
                            #[cfg(target_arch = "x86_64")]
                            let rsp_after = frame.rsp as usize;
                            #[cfg(target_arch = "aarch64")]
                            let rsp_after = frame.sp as usize;
                            debug::emit(DebugFilter::SIGNAL, &DebugEvent::SignalStackWrite {
                                pid,
                                signal,
                                write_what: "trampoline+retaddr",
                                user_addr: rsp_after,
                                len: rsp_before - rsp_after,
                                user_rsp_before: rsp_before,
                                user_rsp_after: rsp_after,
                            });
                        }

                        result?;
                    }
                }
        }

        Ok(())
    }

    /// So-called `sigreturn`: restores the user context when the signal is
    /// delivered to a signal handler.
    pub fn restore_signaled_user_stack(current: &Arc<Process>, current_frame: &mut PtRegs) {
        if let Some(signaled_frame) = current.signaled_frame.swap(None) {
            current
                .arch
                .setup_sigreturn_stack(current_frame, &signaled_frame);
        } else {
            // The user intentionally called sigreturn(2) while it is not signaled.
            // TODO: Should we ignore instead of the killing the process?
            Process::exit_by_signal(SIGKILL);
        }
    }

    /// Creates a new virtual memory space, loads the executable, and overwrites
    /// the **current** process.
    ///
    /// It modifies `frame` to start from the new executable's entry point with
    /// new stack (ie. argv and envp) when the system call handler returns into
    /// the userspace.
    pub fn execve(
        frame: &mut PtRegs,
        executable_path: Arc<PathComponent>,
        argv: &[&[u8]],
        envp: &[&[u8]],
    ) -> Result<()> {
        let current = current_process();
        current.opened_files.lock().close_cloexec_files();
        current.cmdline.borrow_mut().set_by_argv(argv);

        let entry = setup_userspace(executable_path, argv, envp, &current.root_fs)?;

        // de_thread: per POSIX, execve terminates all other threads in the
        // thread group.  Kill siblings NOW — after setup_userspace succeeds
        // (point of no return) but BEFORE replacing the address space.
        //
        // Each sibling's Arc<SpinLock<VirtualMemory>> keeps the OLD page table
        // alive until gc_exited_processes() drops the Arc, so there is no
        // use-after-free even if a sibling is still executing on another CPU
        // for a few hundred nanoseconds after we mark it ExitedWith.
        {
            let tgid = current.tgid;
            let siblings: Vec<Arc<Process>> = {
                let table = PROCESSES.lock();
                table.values()
                    .filter(|p| p.tgid == tgid && p.pid != current.pid)
                    .cloned()
                    .collect()
            };
            for sibling in siblings {
                sibling.set_state(ProcessState::ExitedWith(0));
                EXITED_PROCESSES.lock().push(sibling.clone());
                SCHEDULER.lock().remove(sibling.pid);
                PROCESSES.lock().remove(&sibling.pid);
            }
        }

        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessExec {
            pid: current.pid().as_i32(),
            argv0: core::str::from_utf8(argv.first().copied().unwrap_or(b"?")).unwrap_or("?"),
            entry: entry.ip.value(),
        });

        // Per POSIX, reset signal handlers to SIG_DFL on exec (handler
        // function pointers from the old address space are no longer valid).
        current.signals.lock().reset_on_exec();
        current.signaled_frame.store(None);

        entry.vm.page_table().switch();
        *current.vm.borrow_mut() = Some(Arc::new(SpinLock::new(entry.vm)));

        current
            .arch
            .setup_execve_stack(frame, entry.ip, entry.user_sp);

        Ok(())
    }

    /// Creates a new process. The calling process (`self`) will be the parent
    /// process of the created process. Returns the created child process.
    pub fn fork(parent: &Arc<Process>, parent_frame: &PtRegs) -> Result<Arc<Process>> {
        // Check cgroup pids.max limit before allocating resources.
        crate::cgroups::pids_controller::check_fork_allowed(&parent.cgroup())?;

        let parent_weak = Arc::downgrade(parent);
        let mut process_table = PROCESSES.lock();
        let pid = alloc_pid(&mut process_table)?;
        let arch = parent.arch.fork(parent_frame);
        let vm = parent.vm().as_ref().unwrap().lock().fork()?;
        let opened_files = parent.opened_files().lock().clone(); // TODO: #88 has to address this
        let process_group = parent.process_group();
        let sig_set = parent.sigset_load();
        let parent_umask = parent.umask.load();

        let child = Arc::new(Process {
            is_idle: false,
            process_group: AtomicRefCell::new(Arc::downgrade(&process_group)),
            pid,
            tgid: pid, // fork creates a new thread group; child becomes its own leader
            state: AtomicCell::new(ProcessState::Runnable),
            parent: parent_weak,
            cmdline: AtomicRefCell::new(parent.cmdline().clone()),
            children: SpinLock::new(Vec::new()),
            vm: AtomicRefCell::new(Some(Arc::new(SpinLock::new(vm)))),
            opened_files: Arc::new(SpinLock::new(opened_files)),
            root_fs: parent.root_fs().clone(),
            arch,
            signals: Arc::new(SpinLock::new(SignalDelivery::new())), // TODO: #88 has to address this
            signal_pending: AtomicU32::new(0),
            signaled_frame: AtomicCell::new(None),
            sigset: AtomicU64::new(sig_set.bits()),
            umask: AtomicCell::new(parent_umask),
            uid: AtomicU32::new(parent.uid.load(Ordering::Relaxed)),
            euid: AtomicU32::new(parent.euid.load(Ordering::Relaxed)),
            gid: AtomicU32::new(parent.gid.load(Ordering::Relaxed)),
            egid: AtomicU32::new(parent.egid.load(Ordering::Relaxed)),
            nice: AtomicI32::new(parent.nice.load(Ordering::Relaxed)),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(parent.comm.lock_no_irq().clone()),
            clear_child_tid: AtomicUsize::new(0), // POSIX: not inherited across fork
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(pid.as_i32()),
        });

        // Inherit parent's cgroup and register child.
        let parent_cg = parent.cgroup();
        *child.cgroup.borrow_mut() = Some(parent_cg.clone());
        parent_cg.member_pids.lock().push(pid);

        // Inherit parent's namespaces and allocate namespace-local PID.
        let parent_ns = parent.namespaces();
        if !parent_ns.pid_ns.is_root() {
            let ns_pid = parent_ns.pid_ns.alloc_ns_pid(pid);
            child.ns_pid.store(ns_pid.as_i32(), Ordering::Relaxed);
        }
        *child.namespaces.borrow_mut() = Some(parent_ns);

        process_group.lock().add(Arc::downgrade(&child));
        parent.children().push(child.clone());
        process_table.insert(pid, child.clone());
        drop(process_table);

        // Enqueue child. sys_fork() will call switch() to let the child
        // run first (child-first scheduling, like Linux).
        SCHEDULER.lock().enqueue(pid);

        FORK_TOTAL.fetch_add(1, Ordering::Relaxed);

        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessFork {
            parent_pid: parent.pid().as_i32(),
            child_pid: pid.as_i32(),
        });

        Ok(child)
    }

    /// Creates a new thread in the same thread group as `parent`.
    /// Called for clone(CLONE_VM | CLONE_THREAD | ...).
    ///
    /// The thread shares the parent's address space, file descriptors,
    /// and signal handlers (all via Arc clone). It gets its own PID (= TID),
    /// kernel stack, and register state starting from `child_stack` at the
    /// clone() return address.
    pub fn new_thread(
        parent: &Arc<Process>,
        frame: &PtRegs,
        child_stack: u64,
        newtls: u64,         // FS base for x86_64 / TPIDR_EL0 for ARM64 (0 = inherit parent's)
        child_tidptr: usize, // CLONE_CHILD_SETTID address (0 = none)
        set_child_tid: bool,
        clear_child_tid: bool,
    ) -> Result<Arc<Process>> {
        let mut process_table = PROCESSES.lock();
        let pid = alloc_pid(&mut process_table)?;

        let fs_base = if newtls != 0 { newtls } else { parent.arch.fsbase() };
        let arch = arch::Process::new_thread(frame, child_stack, fs_base);

        let child = Arc::new(Process {
            is_idle: false,
            process_group: AtomicRefCell::new(parent.process_group.borrow().clone()),
            pid,
            tgid: parent.tgid,   // same thread group
            state: AtomicCell::new(ProcessState::Runnable),
            parent: Arc::downgrade(parent),
            cmdline: AtomicRefCell::new(parent.cmdline().clone()),
            children: SpinLock::new(Vec::new()),
            // Share address space, fds, and signal handlers.
            vm: AtomicRefCell::new(parent.vm().as_ref().map(Arc::clone)),
            opened_files: Arc::clone(&parent.opened_files),
            root_fs: parent.root_fs.clone(),
            signals: Arc::clone(&parent.signals),
            signal_pending: AtomicU32::new(0),
            signaled_frame: AtomicCell::new(None),
            sigset: AtomicU64::new(parent.sigset_load().bits()),
            umask: AtomicCell::new(parent.umask.load()),
            uid: AtomicU32::new(parent.uid.load(Ordering::Relaxed)),
            euid: AtomicU32::new(parent.euid.load(Ordering::Relaxed)),
            gid: AtomicU32::new(parent.gid.load(Ordering::Relaxed)),
            egid: AtomicU32::new(parent.egid.load(Ordering::Relaxed)),
            nice: AtomicI32::new(parent.nice.load(Ordering::Relaxed)),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(parent.comm.lock_no_irq().clone()),
            clear_child_tid: AtomicUsize::new(0),
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(pid.as_i32()),
            arch,
        });

        if set_child_tid && child_tidptr != 0 {
            // Write child TID to child's address space (CLONE_CHILD_SETTID).
            if let Ok(uaddr) = UserVAddr::new_nonnull(child_tidptr) {
                let _ = uaddr.write::<i32>(&pid.as_i32());
            }
        }
        if clear_child_tid && child_tidptr != 0 {
            child.clear_child_tid.store(child_tidptr, Ordering::Relaxed);
        }

        parent.process_group().lock().add(Arc::downgrade(&child));
        process_table.insert(pid, child.clone());
        drop(process_table); // Release PROCESSES before acquiring SCHEDULER (lock ordering: SCHEDULER → PROCESSES in switch())
        SCHEDULER.lock().enqueue(pid);

        FORK_TOTAL.fetch_add(1, Ordering::Relaxed);

        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessFork {
            parent_pid: parent.pid().as_i32(),
            child_pid: pid.as_i32(),
        });

        Ok(child)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        trace!(
            "dropping {:?} (cmdline={})",
            self.pid(),
            self.cmdline().as_str()
        );

        // Since the process's reference count has already reached to zero (that's
        // why the process is being dropped), ProcessGroup::remove_dropped_processes
        // should remove this process from its list.
        self.process_group().lock().remove_dropped_processes();
    }
}

/// Walk up the parent chain to find the nearest subreaper, or fall back to
/// init (PID 1). Used when reparenting orphaned children on process exit.
fn find_subreaper_or_init(exiting: &Process) -> Arc<Process> {
    let mut ancestor = exiting.parent.upgrade();
    while let Some(p) = ancestor {
        if p.is_child_subreaper() {
            return p;
        }
        ancestor = p.parent.upgrade();
    }
    // Fall back to init (PID 1).
    PROCESSES.lock().get(&PId::new(1)).cloned()
        .expect("init process (PID 1) must exist")
}

struct UserspaceEntry {
    vm: Vm,
    ip: UserVAddr,
    user_sp: UserVAddr,
}

fn setup_userspace(
    executable_path: Arc<PathComponent>,
    argv: &[&[u8]],
    envp: &[&[u8]],
    root_fs: &Arc<SpinLock<RootFs>>,
) -> Result<UserspaceEntry> {
    do_setup_userspace(executable_path, argv, envp, root_fs, true)
}

fn do_script_binfmt(
    executable_path: &Arc<PathComponent>,
    script_argv: &[&[u8]],
    envp: &[&[u8]],
    root_fs: &Arc<SpinLock<RootFs>>,
    buf: &[u8],
) -> Result<UserspaceEntry> {
    // Set up argv[] with the interpreter and its arguments from the shebang line.
    let mut argv: Vec<&[u8]> = buf[2..buf.iter().position(|&ch| ch == b'\n').unwrap()]
        .split(|&ch| ch == b' ')
        .collect();
    if argv.is_empty() {
        return Err(Errno::EINVAL.into());
    }

    // Push the path to the script file as the first argument to the
    // interpreter.
    let executable_pathbuf = executable_path.resolve_absolute_path();
    argv.push(executable_pathbuf.as_str().as_bytes());

    // Push the original arguments to the script on after the new script
    // invocation (leaving out argv[0] of the previous path of invoking the
    // script.)
    for arg in script_argv.iter().skip(1) {
        argv.push(arg);
    }

    let shebang_path = root_fs.lock().lookup_path(
        Path::new(core::str::from_utf8(argv[0]).map_err(|_| Error::new(Errno::EINVAL))?),
        true,
    )?;

    do_setup_userspace(shebang_path, &argv, envp, root_fs, false)
}

/// Load PT_LOAD segments from an ELF into the VM, then fill inter-segment
/// gaps with anonymous VMAs so that addresses within the full page-aligned
/// span are always backed by a VMA.  This is required because libc (e.g.
/// musl's `reclaim_gaps`) reuses these gap pages for its allocator.
fn load_elf_segments(
    vm: &mut Vm,
    phdrs: &[ProgramHeader],
    base_offset: usize,
    file: &Arc<dyn FileLike>,
) -> Result<()> {
    use kevlar_utils::alignment::align_down;

    // First, add file-backed VMAs for each PT_LOAD (non-page-aligned, as the
    // page fault handler already handles partial pages correctly).
    let mut page_ranges: Vec<(usize, usize)> = Vec::new();
    for phdr in phdrs {
        if phdr.p_type != PT_LOAD {
            continue;
        }

        let seg_start = (phdr.p_vaddr as usize) + base_offset;
        let area_type = if phdr.p_filesz > 0 {
            VmAreaType::File {
                file: file.clone(),
                offset: phdr.p_offset as usize,
                file_size: phdr.p_filesz as usize,
            }
        } else {
            VmAreaType::Anonymous
        };
        vm.add_vm_area(
            UserVAddr::new_nonnull(seg_start)?,
            phdr.p_memsz as usize,
            area_type,
        )?;

        // Track the page-aligned range this segment occupies.
        let seg_end = seg_start + phdr.p_memsz as usize;
        let page_start = align_down(seg_start, PAGE_SIZE);
        let page_end = align_up(seg_end, PAGE_SIZE);
        page_ranges.push((page_start, page_end));
    }

    // Sort by start address.
    page_ranges.sort_by_key(|&(start, _)| start);

    // Fill all gaps with anonymous VMAs: both gaps between segments and
    // the padding after each segment's memsz up to its page-aligned end.
    // We merge all page ranges and fill any unmapped holes.
    let mut covered_end = if let Some(&(start, _)) = page_ranges.first() {
        start
    } else {
        return Ok(());
    };

    for &(page_start, page_end) in &page_ranges {
        // Gap before this segment's page range (between previous segment end
        // and this segment start).
        if covered_end < page_start {
            let gap_len = page_start - covered_end;
            if vm.is_free_vaddr_range(UserVAddr::new_nonnull(covered_end)?, gap_len) {
                vm.add_vm_area(
                    UserVAddr::new_nonnull(covered_end)?,
                    gap_len,
                    VmAreaType::Anonymous,
                )?;
            }
        }
        if page_end > covered_end {
            covered_end = page_end;
        }
    }

    // Now fill gaps within each segment's page range where the VMA
    // doesn't cover the full page-aligned extent.  We walk through the
    // page range and add anonymous VMAs for any bytes not already covered
    // by the file-backed VMAs above.
    for phdr in phdrs {
        if phdr.p_type != PT_LOAD {
            continue;
        }
        let seg_start = (phdr.p_vaddr as usize) + base_offset;
        let seg_end = seg_start + phdr.p_memsz as usize;
        let page_start = align_down(seg_start, PAGE_SIZE);
        let page_end = align_up(seg_end, PAGE_SIZE);

        // Anonymous padding before segment start (within the first page).
        if page_start < seg_start {
            let pad_len = seg_start - page_start;
            if vm.is_free_vaddr_range(UserVAddr::new_nonnull(page_start)?, pad_len) {
                vm.add_vm_area(
                    UserVAddr::new_nonnull(page_start)?,
                    pad_len,
                    VmAreaType::Anonymous,
                )?;
            }
        }

        // Anonymous padding after segment end (within the last page).
        if seg_end < page_end {
            let pad_len = page_end - seg_end;
            if vm.is_free_vaddr_range(UserVAddr::new_nonnull(seg_end)?, pad_len) {
                vm.add_vm_area(
                    UserVAddr::new_nonnull(seg_end)?,
                    pad_len,
                    VmAreaType::Anonymous,
                )?;
            }
        }
    }

    Ok(())
}

fn do_elf_binfmt(
    executable: &Arc<dyn FileLike>,
    argv: &[&[u8]],
    envp: &[&[u8]],
    file_header_pages: kevlar_api::address::PAddr,
    buf: &[u8],
    root_fs: &Arc<SpinLock<RootFs>>,
) -> Result<UserspaceEntry> {
    let file_header_top = USER_STACK_TOP;
    let elf = Elf::parse(buf)?;

    trace!("do_elf_binfmt: e_type={}, is_dyn={}", elf.header().e_type, elf.is_dyn());

    // Check for PT_INTERP (dynamic linker).
    let interp_path = elf.interp_path(buf).map(|s| {
        let mut v = Vec::new();
        v.extend_from_slice(s.as_bytes());
        v
    });

    // For ET_DYN (PIE), compute the address span; for ET_EXEC, compute end_of_image directly.
    let mut main_lo = usize::MAX;
    let mut main_hi = 0usize;
    for phdr in elf.program_headers() {
        if phdr.p_type == PT_LOAD {
            main_lo = core::cmp::min(main_lo, phdr.p_vaddr as usize);
            main_hi = core::cmp::max(main_hi, (phdr.p_vaddr + phdr.p_memsz) as usize);
        }
    }
    if main_lo == usize::MAX {
        main_lo = 0;
    }

    let mut random_bytes = [0u8; 16];
    read_secure_random(((&mut random_bytes) as &mut [u8]).into())?;

    // Build auxiliary vectors. We'll add AT_BASE and AT_ENTRY if we have an interpreter.
    let mut auxv = Vec::new();
    auxv.push(Auxv::Phdr(
        file_header_top
            .sub(buf.len())
            .add(elf.header().e_phoff as usize),
    ));
    auxv.push(Auxv::Phnum(elf.program_headers().len()));
    auxv.push(Auxv::Phent(size_of::<ProgramHeader>()));
    auxv.push(Auxv::Pagesz(PAGE_SIZE));
    auxv.push(Auxv::Hwcap(0)); // no extended HW capabilities (glibc reads this at startup)
    auxv.push(Auxv::Clktck(100)); // TICK_HZ — used by glibc's times()/clock()
    auxv.push(Auxv::Uid(0));
    auxv.push(Auxv::Euid(0));
    auxv.push(Auxv::Gid(0));
    auxv.push(Auxv::Egid(0));
    auxv.push(Auxv::Secure(0));
    auxv.push(Auxv::Random(random_bytes));

    // vDSO: map a shared page with __vdso_clock_gettime for fast userspace clocks.
    #[cfg(target_arch = "x86_64")]
    if let Some(_vdso_paddr) = kevlar_platform::arch::vdso::page_paddr() {
        let vdso_vaddr = kevlar_platform::arch::vdso::VDSO_VADDR;
        auxv.push(Auxv::SysinfoEhdr(UserVAddr::new(vdso_vaddr).unwrap()));
    }

    // Determine base offset for main executable.
    // ET_EXEC: segments are at fixed addresses (main_base_offset = 0).
    // ET_DYN (PIE): segments need relocation; we choose a base.
    let main_span = align_up(main_hi - main_lo, PAGE_SIZE);
    let is_pie = elf.is_dyn();

    // --- Create VM ---
    // For PIE, heap goes right after the relocated image; for ET_EXEC, after the fixed image.
    const USER_STACK_LEN: usize = 128 * 1024;
    let file_header_top_val = file_header_top;
    let init_stack_top = file_header_top_val.sub(buf.len());
    // Heap bottom will be set after we know where the main image lands.

    let ip;
    let mut vm;

    if let Some(ref interp_path_bytes) = interp_path {
        // --- Dynamic linking path ---
        let interp_path_str = core::str::from_utf8(interp_path_bytes)
            .map_err(|_| Error::new(Errno::ENOEXEC))?;
        trace!("loading interpreter: {}", interp_path_str);

        let interp_component = root_fs.lock().lookup_path(Path::new(interp_path_str), true)?;
        let interp_file = interp_component.inode.as_file()?;

        // Read interpreter's ELF header.
        let interp_header_pages = alloc_pages(1, AllocPageFlags::KERNEL)?;
        #[allow(unsafe_code)]
        let interp_buf = unsafe {
            core::slice::from_raw_parts_mut(interp_header_pages.as_mut_ptr(), PAGE_SIZE)
        };
        interp_file.read(0, interp_buf.into(), &OpenOptions::readwrite())?;

        let interp_elf = Elf::parse(interp_buf)?;
        trace!("interpreter parsed: is_dyn={}, entry={:#x}", interp_elf.is_dyn(), interp_elf.entry_offset());
        if !interp_elf.is_dyn() {
            warn!("interpreter is not ET_DYN");
            return Err(Errno::ENOEXEC.into());
        }

        // Compute the interpreter's virtual address span.
        let mut interp_lo = usize::MAX;
        let mut interp_hi = 0usize;
        for phdr in interp_elf.program_headers() {
            if phdr.p_type == PT_LOAD {
                interp_lo = core::cmp::min(interp_lo, phdr.p_vaddr as usize);
                interp_hi = core::cmp::max(interp_hi, (phdr.p_vaddr + phdr.p_memsz) as usize);
            }
        }
        let interp_span = align_up(interp_hi - interp_lo, PAGE_SIZE);
        let interp_entry_offset = interp_elf.entry_offset() as usize;
        let interp_phdrs: Vec<ProgramHeader> = interp_elf.program_headers().to_vec();

        // Create VM. For PIE, we need a temporary heap bottom that gets updated
        // after images are loaded. For ET_EXEC, we know the final address.
        // We'll set a conservative initial heap bottom and update it after loading.
        let user_heap_bottom = if is_pie {
            // PIE: use a high placeholder. alloc_vaddr_range will allocate in the
            // valloc region (high addresses), so heap goes after them.
            // We'll update this after loading all segments.
            align_up(0x10000, PAGE_SIZE) // temporary, will be updated
        } else {
            align_up(main_hi, PAGE_SIZE)
        };
        let user_stack_bottom = init_stack_top.sub(USER_STACK_LEN).value();

        vm = Vm::new(
            UserVAddr::new(user_stack_bottom).unwrap(),
            UserVAddr::new(user_heap_bottom).unwrap(),
        )?;

        // Map file header pages (for AT_PHDR).
        for i in 0..(buf.len() / PAGE_SIZE) {
            vm.page_table_mut().map_user_page(
                file_header_top_val.sub(((buf.len() / PAGE_SIZE) - i) * PAGE_SIZE),
                file_header_pages.add(i * PAGE_SIZE),
            );
        }

        // Load main executable's PT_LOAD segments.
        let main_base_offset = if is_pie {
            let base = vm.alloc_vaddr_range(main_span)?;
            trace!("PIE main: base={:#x}, main_lo={:#x}, main_hi={:#x}, span={:#x}",
                   base.value(), main_lo, main_hi, main_span);
            base.value() - main_lo
        } else {
            0
        };

        let main_entry = (elf.header().e_entry as usize) + main_base_offset;

        load_elf_segments(&mut vm, elf.program_headers(), main_base_offset, executable)?;

        // Allocate address range for interpreter and load its segments.
        let interp_base_uaddr = vm.alloc_vaddr_range(interp_span)?;
        let interp_base_offset = interp_base_uaddr.value() - interp_lo;
        trace!("interpreter: base={:#x}, interp_lo={:#x}, interp_hi={:#x}, offset={:#x}",
               interp_base_uaddr.value(), interp_lo, interp_hi, interp_base_offset);

        load_elf_segments(&mut vm, &interp_phdrs, interp_base_offset, &interp_file)?;

        // Entry point is the interpreter's entry, relocated.
        ip = UserVAddr::new_nonnull(interp_entry_offset + interp_base_offset)?;

        // Update AT_PHDR for PIE: point into the relocated main executable image.
        // The dynamic linker computes load bias as (AT_PHDR - phdr[0].p_vaddr).
        // For PIE, phdrs are at main_base + e_phoff (within the first PT_LOAD segment).
        if is_pie {
            let phdr_addr = main_base_offset + (elf.header().e_phoff as usize);
            trace!("AT_PHDR (PIE relocated): {:#x}", phdr_addr);
            auxv[0] = Auxv::Phdr(UserVAddr::new_nonnull(phdr_addr)?);
        }

        // Add AT_ENTRY (main exe relocated entry) and AT_BASE (interpreter base).
        auxv.push(Auxv::Entry(main_entry));
        auxv.push(Auxv::Base(interp_base_uaddr.value()));

        // Update heap bottom to be after all loaded images.
        // For PIE, both the main exe and interpreter are in the valloc region.
        let final_top = core::cmp::max(
            main_base_offset + main_hi,
            interp_base_offset + interp_hi,
        );
        let new_heap_bottom = align_up(final_top, PAGE_SIZE);
        vm.set_heap_bottom(UserVAddr::new_nonnull(new_heap_bottom)?);

        let phdr_val = match &auxv[0] {
            Auxv::Phdr(v) => v.value(),
            _ => 0,
        };
        warn!("dynamic link: ip={:#x} main_entry={:#x} AT_BASE={:#x} AT_PHDR={:#x} heap={:#x} main_base_offset={:#x}",
              ip.value(), main_entry, interp_base_uaddr.value(),
              phdr_val, new_heap_bottom, main_base_offset);
    } else {
        // --- Static executable (no interpreter) ---
        let end_of_image = main_hi;
        let user_heap_bottom = align_up(end_of_image, PAGE_SIZE);
        let user_stack_bottom = init_stack_top.sub(USER_STACK_LEN).value();

        if user_heap_bottom >= user_stack_bottom {
            return Err(Errno::E2BIG.into());
        }

        vm = Vm::new(
            UserVAddr::new(user_stack_bottom).unwrap(),
            UserVAddr::new(user_heap_bottom).unwrap(),
        )?;

        // Map file header pages.
        for i in 0..(buf.len() / PAGE_SIZE) {
            vm.page_table_mut().map_user_page(
                file_header_top_val.sub(((buf.len() / PAGE_SIZE) - i) * PAGE_SIZE),
                file_header_pages.add(i * PAGE_SIZE),
            );
        }

        // Register main executable's PT_LOAD segments.
        load_elf_segments(&mut vm, elf.program_headers(), 0, executable)?;

        ip = elf.entry()?;
    }

    // Map vDSO page (read + execute, no write) into the new address space.
    #[cfg(target_arch = "x86_64")]
    if let Some(vdso_paddr) = kevlar_platform::arch::vdso::page_paddr() {
        let vdso_uaddr = UserVAddr::new(kevlar_platform::arch::vdso::VDSO_VADDR).unwrap();
        vm.page_table_mut().map_user_page_with_prot(vdso_uaddr, vdso_paddr, 5); // PROT_READ|PROT_EXEC
    }

    // Build init stack.
    let init_stack_len = align_up(estimate_user_init_stack_size(argv, envp, &auxv), PAGE_SIZE);
    let init_stack_pages = alloc_pages(init_stack_len / PAGE_SIZE, AllocPageFlags::KERNEL)?;
    let user_sp = init_user_stack(
        init_stack_top,
        init_stack_pages.as_vaddr().add(init_stack_len),
        init_stack_pages.as_vaddr(),
        argv,
        envp,
        &auxv,
    )?;
    for i in 0..(init_stack_len / PAGE_SIZE) {
        vm.page_table_mut().map_user_page(
            init_stack_top.sub(((init_stack_len / PAGE_SIZE) - i) * PAGE_SIZE),
            init_stack_pages.add(i * PAGE_SIZE),
        );
    }

    Ok(UserspaceEntry { vm, ip, user_sp })
}

/// Creates a new virtual memory space, parses and maps an executable file,
/// and set up the user stack.
fn do_setup_userspace(
    executable_path: Arc<PathComponent>,
    argv: &[&[u8]],
    envp: &[&[u8]],
    root_fs: &Arc<SpinLock<RootFs>>,
    handle_shebang: bool,
) -> Result<UserspaceEntry> {
    // Read the ELF header in the executable file.
    let file_header_len = PAGE_SIZE;
    let file_header_pages = alloc_pages(file_header_len / PAGE_SIZE, AllocPageFlags::KERNEL)?;
    #[allow(unsafe_code)]
    let buf =
        unsafe { core::slice::from_raw_parts_mut(file_header_pages.as_mut_ptr(), file_header_len) };

    let executable = executable_path.inode.as_file()?;
    executable.read(0, buf.into(), &OpenOptions::readwrite())?;

    if handle_shebang && buf.starts_with(b"#!") && buf.contains(&b'\n') {
        return do_script_binfmt(&executable_path, argv, envp, root_fs, buf);
    }

    do_elf_binfmt(executable, argv, envp, file_header_pages, buf, root_fs)
}

pub fn gc_exited_processes() {
    if current_process().is_idle() {
        // If we're in an idle thread, it's safe to free kernel stacks allocated
        // for other exited processes.
        EXITED_PROCESSES.lock().clear();
    }
}
