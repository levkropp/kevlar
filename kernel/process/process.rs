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
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
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
pub(super) static EXITED_PROCESSES: SpinLock<Vec<Arc<Process>>> = SpinLock::new(Vec::new());

static FORK_TOTAL: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
pub struct Stats {
    pub fork_total: usize,
}

pub fn process_count() -> usize {
    PROCESSES.lock().len()
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
            root_fs: INITIAL_ROOT_FS.clone(),
            opened_files: Arc::new(SpinLock::new(OpenedFileTable::new())),
            signals: Arc::new(SpinLock::new(SignalDelivery::new())),
            signal_pending: AtomicU32::new(0),
            signaled_frame: AtomicCell::new(None),
            sigset: AtomicU64::new(0),
            umask: AtomicCell::new(0o022),
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

        debug_assert!(!matches!(old_state, ProcessState::ExitedWith(_)));

        if old_state == ProcessState::Runnable {
            return;
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
            info!("init exited with status {}, halting system", status);
            kevlar_platform::arch::halt();
        }

        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessExit {
            pid: current.pid().as_i32(),
            status,
            by_signal: false,
        });

        current.set_state(ProcessState::ExitedWith(status));
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

        // Close opened files here instead of in Drop::drop because `proc` is
        // not dropped until it's joined by the parent process. Drop them to
        // make pipes closed.
        current.opened_files.lock().close_all();

        PROCESSES.lock().remove(&current.pid);
        JOIN_WAIT_QUEUE.wake_all();
        switch();
        unreachable!();
    }

    /// Terminates the **current** thread and other threads belonging to the same thread group.
    pub fn exit_group(status: c_int) -> ! {
        // TODO: Kill other threads belonging to the same thread group.
        Process::exit(status)
    }

    /// Terminates the **current** process by a signal.
    pub fn exit_by_signal(_signal: Signal) -> ! {
        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessExit {
            pid: current_process().pid().as_i32(),
            status: 128 + _signal,
            by_signal: true,
        });
        Process::exit(1 /* FIXME: how should we compute the exit status? */);
    }

    /// Sends a signal.
    pub fn send_signal(&self, signal: Signal) {
        // SIGCONT always continues a stopped process, even if SIGCONT is blocked.
        if signal == SIGCONT {
            self.continue_process();
        }

        self.signals.lock().signal(signal);
        self.signal_pending.fetch_or(1 << signal, Ordering::Release);
        self.resume();
    }

    /// Returns `true` if there's a pending signal.
    pub fn has_pending_signals(&self) -> bool {
        self.signal_pending.load(Ordering::Relaxed) != 0
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
            let result = sigs.pop_pending();
            // Sync the atomic mirror with the actual pending state.
            if sigs.is_pending() {
                // Still more signals pending — leave the flag set.
            } else {
                current.signal_pending.store(0, Ordering::Relaxed);
            }
            result
        };
        if let Some((signal, sigaction)) = popped {
            let sigset = current.sigset_load();
            if !sigset.is_blocked(signal as usize) {
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
                    SigAction::Handler { handler } => {
                        let rsp_before = frame.rsp as usize;
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
                        let result = current.arch.setup_signal_stack(frame, signal, handler);
                        debug::usercopy::clear_context();

                        // Emit detailed signal stack write trace.
                        if debug::is_enabled(DebugFilter::USERCOPY) || debug::is_enabled(DebugFilter::SIGNAL) {
                            let rsp_after = frame.rsp as usize;
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
        });

        process_group.lock().add(Arc::downgrade(&child));
        parent.children().push(child.clone());
        process_table.insert(pid, child.clone());
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

        // Create VM. For PIE, heap bottom is arbitrary (will be after the alloc region).
        // Use a safe default; the actual images are placed via alloc_vaddr_range.
        let user_heap_bottom = if is_pie {
            // PIE: heap starts after a placeholder. Images go in the valloc region.
            align_up(PAGE_SIZE, PAGE_SIZE)
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

        trace!("dynamic link: ip={:#x}, main_entry={:#x}, interp_base={:#x}",
              ip.value(), main_entry, interp_base_uaddr.value());
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
