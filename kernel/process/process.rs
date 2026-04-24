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
        signal::{SigAction, SigSet, Signal, SignalDelivery, SignalMask, SIGCHLD, SIGCONT, SIGKILL, SIGSTOP},
        switch, UserVAddr, JOIN_WAIT_QUEUE, SCHEDULER, SchedulerPolicy, WaitQueue,
    },
    random::read_secure_random,
    result::Errno,
    INITIAL_ROOT_FS,
};
use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use arrayvec::ArrayString;
use atomic_refcell::{AtomicRef, AtomicRefCell};
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use core::sync::atomic::AtomicUsize;
use crossbeam::atomic::AtomicCell;
use goblin::elf64::program_header::PT_LOAD;
use kevlar_platform::{
    arch::{PtRegs, PAGE_SIZE},
    page_allocator::{alloc_pages, alloc_page_batch, AllocPageFlags},
    spinlock::{SpinLock, SpinLockGuard, SpinLockGuardNoIrq},
};
use kevlar_utils::alignment::align_up;

// ── Per-process syscall trace ring buffer ────────────────────────────────
//
// Records the last 32 syscalls per process for crash diagnostics.
// Lock-free: uses an atomic write index with Relaxed ordering.
// Each entry is 16 bytes (nr: u16, pad: u16, arg0: u32, arg1: u32, result: i32).

const SYSCALL_TRACE_SIZE: usize = 32;

/// A single entry in the syscall trace ring buffer.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct SyscallTraceEntry {
    pub nr: u16,
    pub _pad: u16,
    pub arg0: u32,
    pub arg1: u32,
    pub result: i32,
}

/// Per-process ring buffer of the last `SYSCALL_TRACE_SIZE` syscalls.
///
/// Lock-free using a single atomic write index. Each `record_syscall()` call
/// is just a few atomic writes — no locks, no allocation.
pub struct SyscallTrace {
    entries: [AtomicCell<SyscallTraceEntry>; SYSCALL_TRACE_SIZE],
    write_idx: AtomicU32,
}

impl SyscallTrace {
    pub fn new() -> Self {
        const EMPTY: SyscallTraceEntry = SyscallTraceEntry {
            nr: 0,
            _pad: 0,
            arg0: 0,
            arg1: 0,
            result: 0,
        };
        // AtomicCell<SyscallTraceEntry> is not Copy, so we initialize with array::from_fn.
        SyscallTrace {
            entries: core::array::from_fn(|_| AtomicCell::new(EMPTY)),
            write_idx: AtomicU32::new(0),
        }
    }

    /// Record a syscall. Called unconditionally from the dispatch path.
    /// Cost: one atomic fetch_add + one AtomicCell store.
    pub fn record(&self, nr: usize, a1: usize, a2: usize, result: isize) {
        let idx = self.write_idx.fetch_add(1, Ordering::Relaxed) as usize % SYSCALL_TRACE_SIZE;
        self.entries[idx].store(SyscallTraceEntry {
            nr: nr as u16,
            _pad: 0,
            arg0: a1 as u32,
            arg1: a2 as u32,
            result: result as i32,
        });
    }

    /// Return the last N entries in chronological order.
    pub fn dump(&self) -> Vec<SyscallTraceEntry> {
        let w = self.write_idx.load(Ordering::Relaxed) as usize;
        let count = core::cmp::min(w, SYSCALL_TRACE_SIZE);
        let mut out = Vec::with_capacity(count);
        // The oldest entry is at (w - count) % SIZE, the newest is at (w - 1) % SIZE.
        for i in 0..count {
            let idx = (w - count + i) % SYSCALL_TRACE_SIZE;
            out.push(self.entries[idx].load());
        }
        out
    }
}

type ProcessTable = BTreeMap<PId, Arc<Process>>;

/// The process table. All processes are registered in with its process Id.
pub(super) static PROCESSES: SpinLock<ProcessTable> = SpinLock::new_ranked(
    BTreeMap::new(),
    kevlar_platform::lockdep::rank::PROCESSES,
    "PROCESSES",
);
pub static EXITED_PROCESSES: SpinLock<Vec<Arc<Process>>> = SpinLock::new_ranked(
    Vec::new(),
    kevlar_platform::lockdep::rank::EXITED_PROCESSES,
    "EXITED_PROCESSES",
);

static FORK_TOTAL: AtomicUsize = AtomicUsize::new(0);

// --- Experiment toggles ---
// Flip these to measure each optimization independently.
// When false, the experiment's code path is bypassed and the original behavior is used.
/// Ghost-fork: duplicate page tables but skip refcount operations, block
/// parent until child exec/exit. Unlike the original ghost-fork (which shared
/// the VM and was incompatible with musl's _Fork() wrapper), this creates a
/// SEPARATE page table with CoW marking. musl's writes trigger CoW faults
/// and copy to private pages — the parent's data is never corrupted.
/// Saves ~8µs per fork by eliminating ~200+ atomic refcount increments.
/// Ghost-fork with targeted restore: page table duplication skips all
/// refcount operations. A bitmap of CoW-marked addresses enables O(N)
/// restore (N=writable pages, ~200) instead of O(all PTEs, ~10K).
/// Ghost-fork with targeted restore via address bitmap. DISABLED because it
/// deadlocks fork+interact patterns (e.g. pipe_pingpong: child writes to pipe,
/// parent blocked in fork → can't read → deadlock). Only safe for fork+exec
/// or fork+exit-immediately, but we can't distinguish at fork() time.
/// Could be enabled as opt-in via posix_spawn() or a dedicated syscall.
pub static GHOST_FORK_ENABLED: AtomicBool = AtomicBool::new(false);
/// Experiment 2: Prefault template (cache prefault mappings, replay on subsequent execs).
pub static PREFAULT_TEMPLATE_ENABLED: AtomicBool = AtomicBool::new(true);
/// Experiment 3: Direct physical mapping (map initramfs pages directly, no copy).
/// Enabled as of blog 221: `kernel/fs/initramfs.rs::align_file_data` now
/// relocates every regular file's bytes into a page-aligned, sentinel-
/// refcounted kernel buffer at boot, so `data_vaddr() % PAGE_SIZE == 0`
/// and the alignment check succeeds for every initramfs file.  Saves
/// the per-fault alloc+copy and lets fork's CoW walk short-circuit the
/// data-page refcount bumps via `PAGE_REF_KERNEL_IMAGE`.
pub static DIRECT_MAP_ENABLED: AtomicBool = AtomicBool::new(true);
/// Wait queue for vfork parents. Woken when the child calls _exit or exec.
pub static VFORK_WAIT_QUEUE: kevlar_utils::once::Once<WaitQueue> = kevlar_utils::once::Once::new();

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
    /// The process has exited normally (via exit(2) or _exit(2)).
    ExitedWith(c_int),
    /// The process was killed by a signal (default Terminate disposition).
    ExitedBySignal(c_int),
}

impl ProcessState {
    /// Pack into u64: high byte is tag, low 32 bits hold the i32 payload
    /// (signal number / exit code). Used by AtomicProcessState.
    #[inline(always)]
    const fn pack(self) -> u64 {
        let (tag, payload) = match self {
            ProcessState::Runnable => (0u8, 0i32),
            ProcessState::BlockedSignalable => (1, 0),
            ProcessState::Stopped(s) => (2, s),
            ProcessState::ExitedWith(c) => (3, c),
            ProcessState::ExitedBySignal(c) => (4, c),
        };
        ((tag as u64) << 32) | (payload as u32 as u64)
    }

    #[inline(always)]
    const fn unpack(raw: u64) -> ProcessState {
        let tag = (raw >> 32) as u8;
        let payload = raw as u32 as i32;
        match tag {
            0 => ProcessState::Runnable,
            1 => ProcessState::BlockedSignalable,
            2 => ProcessState::Stopped(payload),
            3 => ProcessState::ExitedWith(payload),
            _ => ProcessState::ExitedBySignal(payload),
        }
    }
}

/// Lock-free atomic ProcessState. Replaces `AtomicCell<ProcessState>`,
/// which fell back to crossbeam's SeqLock (since Rust can't prove a
/// non-primitive enum is lock-free-atomic). The SeqLock livelocked
/// under heavy process churn from `scan_suspended_task_corruption`.
/// Manual u64 packing sidesteps the problem — ProcessState fits in
/// 5 tags × i32 payload.
pub struct AtomicProcessState(core::sync::atomic::AtomicU64);

impl AtomicProcessState {
    pub const fn new(state: ProcessState) -> Self {
        AtomicProcessState(core::sync::atomic::AtomicU64::new(state.pack()))
    }
    #[inline(always)]
    pub fn load(&self) -> ProcessState {
        ProcessState::unpack(self.0.load(core::sync::atomic::Ordering::Acquire))
    }
    #[inline(always)]
    pub fn store(&self, state: ProcessState) {
        self.0.store(state.pack(), core::sync::atomic::Ordering::Release);
    }
    #[inline(always)]
    pub fn swap(&self, state: ProcessState) -> ProcessState {
        ProcessState::unpack(self.0.swap(state.pack(), core::sync::atomic::Ordering::AcqRel))
    }
}

/// Build a pre-computed `struct utsname` (390 bytes) from a UTS namespace.
/// Used to cache the result so sys_uname becomes a single memcpy.
fn build_cached_utsname(uts: &crate::namespace::UtsNamespace) -> [u8; 390] {
    let mut buf = [0u8; 390];
    #[inline(always)]
    fn write_field(buf: &mut [u8; 390], idx: usize, value: &[u8]) {
        let offset = idx * 65;
        let len = value.len().min(64);
        buf[offset..offset + len].copy_from_slice(&value[..len]);
    }
    write_field(&mut buf, 0, b"Linux");    // sysname
    write_field(&mut buf, 2, b"6.19.8");   // release
    write_field(&mut buf, 3, b"Kevlar");   // version
    #[cfg(target_arch = "x86_64")]
    write_field(&mut buf, 4, b"x86_64");   // machine
    #[cfg(target_arch = "aarch64")]
    write_field(&mut buf, 4, b"aarch64");
    uts.write_hostname_into(&mut buf, 1);
    uts.write_domainname_into(&mut buf, 5);
    buf
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
    /// Session ID — PID of the session leader. Set by setsid(), inherited by fork().
    session_id: AtomicI32,
    state: AtomicProcessState,
    parent: Weak<Process>,
    cmdline: AtomicRefCell<Cmdline>,
    children: SpinLock<Vec<Arc<Process>>>,
    vm: AtomicRefCell<Option<Arc<SpinLock<Vm>>>>,
    opened_files: Arc<SpinLock<OpenedFileTable>>,
    root_fs: AtomicRefCell<Arc<SpinLock<RootFs>>>,
    signals: Arc<SpinLock<SignalDelivery>>,
    /// Lock-free mirror of `signals.pending`.  Avoids taking the spinlock on
    /// every syscall exit when no signals are pending (the common case).
    signal_pending: AtomicU32,
    /// Stack of saved register contexts for nested signal delivery.
    /// Each signal handler pushes its interrupted context; rt_sigreturn pops it.
    ///
    /// Lazy-allocated (None until first signal delivery) so fork() doesn't pay
    /// ~1 KB of inline zero-init — rare signal-handler use didn't warrant that
    /// cost on every process.  See blog 216.
    signaled_frame_stack: SpinLock<Option<alloc::boxed::Box<arrayvec::ArrayVec<PtRegs, 4>>>>,
    /// Stack of user-stack context base addresses for nested signal delivery.
    /// Paired with signaled_frame_stack: setup_signal_stack pushes ctx_base,
    /// setup_sigreturn_stack pops it. Needed when signal is delivered on an
    /// alternate stack (SA_ONSTACK) since signaled_frame.rsp is the pre-switch RSP.
    signal_ctx_base_stack: SpinLock<Option<alloc::boxed::Box<arrayvec::ArrayVec<usize, 4>>>>,
    sigset: AtomicU64,
    umask: AtomicCell<u32>,
    // UID/GID tracking (Phase 5) + saved IDs (M10.4).
    uid: AtomicU32,
    euid: AtomicU32,
    suid: AtomicU32,
    gid: AtomicU32,
    egid: AtomicU32,
    sgid: AtomicU32,
    /// Nice value (-20 to +19). Used by getpriority/setpriority.
    nice: AtomicI32,
    /// Whether this process is a child subreaper (PR_SET_CHILD_SUBREAPER).
    is_child_subreaper: AtomicBool,
    /// Process name set via PR_SET_NAME (max 16 bytes including NUL).
    comm: SpinLock<Option<Vec<u8>>>,
    /// Address to write 0 to (and then wake the futex) when this thread exits.
    /// Set by `set_tid_address(2)`. Used by pthread_join via futex.
    clear_child_tid: AtomicUsize,
    /// If this process was created by vfork, the parent's PID.
    /// When this process calls _exit or exec, the parent is woken.
    vfork_parent: Option<PId>,
    /// Monotonic tick count at process creation (for /proc/[pid]/stat field 22).
    start_ticks: u64,
    /// Accumulated user-mode ticks (incremented by timer IRQ).
    utime: AtomicU64,
    /// Accumulated kernel-mode ticks (incremented per syscall).
    stime: AtomicU64,
    /// Supplementary group IDs (set by setgroups, inherited on fork).
    groups: SpinLock<Vec<u32>>,
    /// cgroup v2 membership (None only for idle threads created before cgroups::init).
    cgroup: atomic_refcell::AtomicRefCell<Option<Arc<crate::cgroups::CgroupNode>>>,
    /// Namespace set (UTS, PID, mount).
    namespaces: AtomicRefCell<Option<crate::namespace::NamespaceSet>>,
    /// Namespace-local PID (equals global PID in root PID namespace).
    ns_pid: AtomicI32,
    /// Per-process syscall trace ring buffer (last 32 syscalls).
    /// Used for crash diagnostics — always recorded, no debug flag needed.
    syscall_trace: SyscallTrace,
    /// Resolved executable path (after symlink resolution).
    /// Used for /proc/[pid]/exe symlink.  Set by execve, inherited by fork.
    exe_path: SpinLock<ArrayString<256>>,
    /// NUL-separated envp as passed to execve (or inherited from parent on
    /// fork). Matches /proc/[pid]/environ format in Linux. Bounded to 8 KiB
    /// to keep per-process memory bounded for degenerate apps.
    environ: SpinLock<alloc::vec::Vec<u8>>,
    /// Set to true when this ghost-fork child has exec'd or exited.
    /// Used by the parent's VFORK_WAIT_QUEUE predicate to detect completion.
    pub ghost_fork_done: AtomicBool,
    /// Saved signal mask from rt_sigsuspend, restored by rt_sigreturn.
    sigsuspend_saved_mask: AtomicU64,
    /// True when `sigsuspend_saved_mask` contains a valid mask to restore.
    sigsuspend_has_mask: AtomicBool,
    /// Alternate signal stack (sigaltstack). sp=0 means not set.
    pub alt_stack_sp: AtomicUsize,
    pub alt_stack_size: AtomicUsize,
    pub alt_stack_flags: AtomicU32,
    /// O3: Cached epoll fd number for hot-path bypass (-1 = invalid).
    #[cfg(not(feature = "profile-fortress"))]
    epoll_hot_fd: AtomicI32,
    /// O3: Raw pointer to EpollInstance for the cached fd (null = invalid).
    #[cfg(not(feature = "profile-fortress"))]
    epoll_hot_ptr: AtomicPtr<u8>,
    /// Cached fd number for with_file hot-path bypass (-1 = invalid).
    /// Caches the last fd→OpenedFile mapping to skip fd table lookup.
    #[cfg(not(feature = "profile-fortress"))]
    file_hot_fd: AtomicI32,
    /// Raw pointer to OpenedFile for the cached fd (null = invalid).
    #[cfg(not(feature = "profile-fortress"))]
    file_hot_ptr: AtomicPtr<u8>,
    /// Per-process resource limits (16 resources × [cur, max]).
    /// Lock-free: reads via AtomicU64, writes via atomic stores.
    rlimits: AtomicRlimits,
    /// Pre-built `struct utsname` (390 bytes) for fast sys_uname response.
    /// Built at init/fork from UTS namespace data. TODO: rebuild on sethostname/setdomainname.
    cached_utsname: SpinLock<[u8; 390]>,
    /// Physical address of this process's personal vDSO data page.
    /// 0 = not yet allocated (before vdso::init() runs).
    #[cfg(target_arch = "x86_64")]
    vdso_data_paddr: AtomicU64,
}

/// Lock-free resource limits: 16 resources × [cur, max] as AtomicU64 pairs.
/// Reads are lock-free (2 atomic loads). Writes are rare (setrlimit only).
struct AtomicRlimits {
    vals: [AtomicU64; 32], // [cur0, max0, cur1, max1, ...]
}

impl AtomicRlimits {
    fn new(init: [[u64; 2]; 16]) -> Self {
        let vals = core::array::from_fn(|i| {
            AtomicU64::new(init[i / 2][i % 2])
        });
        Self { vals }
    }

    fn get(&self, idx: usize) -> [u64; 2] {
        [self.vals[idx * 2].load(Ordering::Relaxed),
         self.vals[idx * 2 + 1].load(Ordering::Relaxed)]
    }

    fn set(&self, idx: usize, cur: u64, max: u64) {
        self.vals[idx * 2].store(cur, Ordering::Relaxed);
        self.vals[idx * 2 + 1].store(max, Ordering::Relaxed);
    }

    fn get_all(&self) -> [[u64; 2]; 16] {
        core::array::from_fn(|i| self.get(i))
    }
}

/// Default resource limits (RLIMIT_* indexed, 16 entries × [cur, max]).
/// Flatten envp into NUL-separated bytes, the format used by
/// /proc/[pid]/environ in Linux. Bounded to 8 KiB to cap per-process
/// memory for degenerate apps with thousands of env vars.
fn envp_to_vec(envp: &[&[u8]]) -> alloc::vec::Vec<u8> {
    const ENVIRON_LIMIT: usize = 8 * 1024;
    let mut out = alloc::vec::Vec::new();
    for var in envp {
        if out.len() + var.len() + 1 > ENVIRON_LIMIT {
            break;
        }
        out.extend_from_slice(var);
        out.push(0);
    }
    out
}

fn default_rlimits() -> [[u64; 2]; 16] {
    const INF: u64 = !0u64;
    let mut rl = [[INF; 2]; 16];
    rl[3] = [8 * 1024 * 1024, INF]; // RLIMIT_STACK: 8MB soft, unlimited hard
    rl[4] = [0, INF];               // RLIMIT_CORE: 0 soft (no core dumps)
    rl[7] = [1024, 4096];           // RLIMIT_NOFILE: 1024 soft, 4096 hard
    rl[13] = [0, 0];                // RLIMIT_NICE: 0
    rl[14] = [0, 0];                // RLIMIT_RTPRIO: 0
    rl
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
            state: AtomicProcessState::new(ProcessState::Runnable),
            parent: Weak::new(),
            cmdline: AtomicRefCell::new(Cmdline::new()),
            environ: SpinLock::new(alloc::vec::Vec::new()),
            children: SpinLock::new(Vec::new()),
            vm: AtomicRefCell::new(None),
            pid: PId::new(0),
            tgid: PId::new(0),
            session_id: AtomicI32::new(0),
            root_fs: AtomicRefCell::new(INITIAL_ROOT_FS.clone()),
            opened_files: Arc::new(SpinLock::new(OpenedFileTable::new())),
            signals: Arc::new(SpinLock::new(SignalDelivery::new())),
            signal_pending: AtomicU32::new(0),
            signaled_frame_stack: SpinLock::new(None),
            signal_ctx_base_stack: SpinLock::new(None),
            sigset: AtomicU64::new(0),
            umask: AtomicCell::new(0o022),
            uid: AtomicU32::new(0),
            euid: AtomicU32::new(0),
            suid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            egid: AtomicU32::new(0),
            sgid: AtomicU32::new(0),
            nice: AtomicI32::new(0),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(None),
            clear_child_tid: AtomicUsize::new(0),
            vfork_parent: None,
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            groups: SpinLock::new(Vec::new()),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(0),
            syscall_trace: SyscallTrace::new(),
            exe_path: SpinLock::new(ArrayString::new()),
            ghost_fork_done: AtomicBool::new(false),
            sigsuspend_saved_mask: AtomicU64::new(0),
            sigsuspend_has_mask: AtomicBool::new(false),
            alt_stack_sp: AtomicUsize::new(0),
            alt_stack_size: AtomicUsize::new(0),
            alt_stack_flags: AtomicU32::new(0),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            rlimits: AtomicRlimits::new(default_rlimits()),
            cached_utsname: SpinLock::new([0u8; 390]),
            #[cfg(target_arch = "x86_64")]
            vdso_data_paddr: AtomicU64::new(0),
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

        let resolved_exe = {
            let p = executable_path.resolve_absolute_path();
            let mut s = ArrayString::<256>::new();
            let _ = s.try_push_str(p.as_str());
            s
        };
        let init_utsname = build_cached_utsname(&*crate::namespace::ROOT_UTS);
        #[cfg(target_arch = "x86_64")]
        let init_vdso_paddr = kevlar_platform::arch::vdso::alloc_process_page(
            1, 1, 0, 0, &init_utsname,
        ).map(|p| p.value() as u64).unwrap_or(0);
        let mut entry = setup_userspace(executable_path, argv, &[], &root_fs)?;
        // Remap vDSO with per-process page for PID 1.
        #[cfg(target_arch = "x86_64")]
        if init_vdso_paddr != 0 {
            let vdso_uaddr = UserVAddr::new(kevlar_platform::arch::vdso::VDSO_VADDR).unwrap();
            entry.vm.page_table_mut().map_user_page_with_prot(
                vdso_uaddr, kevlar_platform::address::PAddr::new(init_vdso_paddr as usize), 5,
            );
        }
        let pid = PId::new(1);
        let process_group = ProcessGroup::new(PgId::new(1));
        let process = Arc::new(Process {
            is_idle: false,
            process_group: AtomicRefCell::new(Arc::downgrade(&process_group)),
            pid,
            tgid: pid,
            session_id: AtomicI32::new(1), // PID 1 is its own session leader
            parent: Weak::new(),
            children: SpinLock::new(Vec::new()),
            state: AtomicProcessState::new(ProcessState::Runnable),
            cmdline: AtomicRefCell::new(Cmdline::from_argv(argv)),
            environ: SpinLock::new(envp_to_vec(&[])),
            arch: arch::Process::new_user_thread(entry.ip, entry.user_sp),
            vm: AtomicRefCell::new(Some(Arc::new(SpinLock::new(entry.vm)))),
            opened_files: Arc::new(SpinLock::new(opened_files)),
            root_fs: AtomicRefCell::new(root_fs),
            signals: Arc::new(SpinLock::new(SignalDelivery::new())),
            signal_pending: AtomicU32::new(0),
            signaled_frame_stack: SpinLock::new(None),
            signal_ctx_base_stack: SpinLock::new(None),
            sigset: AtomicU64::new(0),
            umask: AtomicCell::new(0o022),
            uid: AtomicU32::new(0),
            euid: AtomicU32::new(0),
            suid: AtomicU32::new(0),
            gid: AtomicU32::new(0),
            egid: AtomicU32::new(0),
            sgid: AtomicU32::new(0),
            nice: AtomicI32::new(0),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(None),
            clear_child_tid: AtomicUsize::new(0),
            vfork_parent: None,
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            groups: SpinLock::new(Vec::new()),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(pid.as_i32()),
            syscall_trace: SyscallTrace::new(),
            exe_path: SpinLock::new(resolved_exe),
            ghost_fork_done: AtomicBool::new(false),
            sigsuspend_saved_mask: AtomicU64::new(0),
            sigsuspend_has_mask: AtomicBool::new(false),
            alt_stack_sp: AtomicUsize::new(0),
            alt_stack_size: AtomicUsize::new(0),
            alt_stack_flags: AtomicU32::new(0),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            rlimits: AtomicRlimits::new(default_rlimits()),
            cached_utsname: SpinLock::new(init_utsname),
            #[cfg(target_arch = "x86_64")]
            vdso_data_paddr: AtomicU64::new(init_vdso_paddr),
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
    #[allow(dead_code)]
    pub fn tgid(&self) -> PId {
        self.tgid
    }

    /// Session ID — PID of the session leader.
    pub fn session_id(&self) -> i32 {
        self.session_id.load(Ordering::Relaxed)
    }

    /// Set session ID (called by setsid).
    pub fn set_session_id(&self, sid: i32) {
        self.session_id.store(sid, Ordering::Relaxed);
    }

    /// Get supplementary group IDs.
    pub fn groups(&self) -> Vec<u32> {
        self.groups.lock_no_irq().clone()
    }

    /// Set supplementary group IDs.
    pub fn set_groups(&self, gids: Vec<u32>) {
        *self.groups.lock_no_irq() = gids;
    }

    /// Get resource limits table (lock-free, reads 32 atomics).
    pub fn rlimits(&self) -> [[u64; 2]; 16] {
        self.rlimits.get_all()
    }

    /// Get a single resource limit pair [cur, max] (lock-free, 2 atomic loads).
    pub fn rlimit(&self, resource: usize) -> [u64; 2] {
        if resource < 16 { self.rlimits.get(resource) } else { [!0u64, !0u64] }
    }

    /// Set a single resource limit (lock-free, 2 atomic stores).
    pub fn set_rlimit(&self, resource: usize, cur: u64, max: u64) {
        if resource < 16 {
            self.rlimits.set(resource, cur, max);
        }
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

    /// Record a syscall in the per-process trace ring buffer.
    /// Called unconditionally from the syscall dispatch path.
    pub fn record_syscall(&self, nr: usize, a1: usize, a2: usize, result: isize) {
        self.syscall_trace.record(nr, a1, a2, result);
    }

    /// Return the last N syscall trace entries in chronological order.
    pub fn dump_trace(&self) -> Vec<SyscallTraceEntry> {
        self.syscall_trace.dump()
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

    /// Returns the resolved executable path (for /proc/[pid]/exe).
    pub fn exe_path_string(&self) -> String {
        String::from(self.exe_path.lock_no_irq().as_str())
    }

    /// Returns a clone of the NUL-separated environ (for /proc/[pid]/environ).
    pub fn environ_clone(&self) -> alloc::vec::Vec<u8> {
        self.environ.lock_no_irq().clone()
    }

    /// Overwrites the stored environ — called from execve.
    pub fn set_environ(&self, envp: &[&[u8]]) {
        let bytes = envp_to_vec(envp);
        let mut guard = self.environ.lock_no_irq();
        *guard = bytes;
    }

    // ── UID/GID accessors ────────────────────────────────────────────
    pub fn uid(&self) -> u32 { self.uid.load(Ordering::Relaxed) }
    pub fn euid(&self) -> u32 { self.euid.load(Ordering::Relaxed) }
    pub fn suid(&self) -> u32 { self.suid.load(Ordering::Relaxed) }
    pub fn gid(&self) -> u32 { self.gid.load(Ordering::Relaxed) }
    pub fn egid(&self) -> u32 { self.egid.load(Ordering::Relaxed) }
    pub fn sgid(&self) -> u32 { self.sgid.load(Ordering::Relaxed) }
    pub fn set_uid(&self, uid: u32) { self.uid.store(uid, Ordering::Relaxed); }
    pub fn set_euid(&self, euid: u32) { self.euid.store(euid, Ordering::Relaxed); }
    pub fn set_suid(&self, suid: u32) { self.suid.store(suid, Ordering::Relaxed); }
    pub fn set_gid(&self, gid: u32) { self.gid.store(gid, Ordering::Relaxed); }
    pub fn set_egid(&self, egid: u32) { self.egid.store(egid, Ordering::Relaxed); }
    pub fn set_sgid(&self, sgid: u32) { self.sgid.store(sgid, Ordering::Relaxed); }
    pub fn nice(&self) -> i32 { self.nice.load(Ordering::Relaxed) }
    pub fn set_nice(&self, n: i32) { self.nice.store(n, Ordering::Relaxed); }

    /// Returns a copy of the pre-built utsname buffer (390 bytes).
    pub fn utsname_copy(&self) -> [u8; 390] {
        *self.cached_utsname.lock_no_irq()
    }

    /// Rebuild the cached utsname from the current UTS namespace.
    /// Called after sethostname/setdomainname to ensure uname(2) returns
    /// the updated values.
    pub fn rebuild_cached_utsname(&self) {
        let ns = self.namespaces();
        *self.cached_utsname.lock_no_irq() = build_cached_utsname(&ns.uts);
    }

    /// Returns the physical address of this process's vDSO data page (0 if none).
    #[cfg(target_arch = "x86_64")]
    pub fn vdso_data_paddr(&self) -> u64 {
        self.vdso_data_paddr.load(Ordering::Relaxed)
    }

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
            let cmdline = self.cmdline();
            let argv0 = cmdline.argv0();
            let basename = argv0.rsplit('/').next().unwrap_or(argv0);
            basename.as_bytes().to_vec()
        }
    }

    /// Its child processes.
    pub fn children(&self) -> SpinLockGuard<'_, Vec<Arc<Process>>> {
        self.children.lock()
    }

    /// The process's path resolution info.
    pub fn root_fs(&self) -> Arc<SpinLock<RootFs>> {
        self.root_fs.borrow().clone()
    }

    /// Give this process its own copy of root_fs (for chroot isolation).
    /// The new root_fs shares mount points but has independent root/cwd.
    pub fn unshare_root_fs(&self) {
        let cloned = self.root_fs.borrow().lock().clone();
        *self.root_fs.borrow_mut() = Arc::new(SpinLock::new(cloned));
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

    /// O3: Read the cached epoll fd number (-1 = no cache).
    #[cfg(not(feature = "profile-fortress"))]
    pub fn epoll_hot_fd(&self) -> i32 {
        self.epoll_hot_fd.load(Ordering::Relaxed)
    }

    /// O3: Read the cached epoll instance pointer (null = no cache).
    #[cfg(not(feature = "profile-fortress"))]
    pub fn epoll_hot_ptr(&self) -> *mut u8 {
        self.epoll_hot_ptr.load(Ordering::Relaxed)
    }

    /// O3: Populate the hot-fd cache.
    #[cfg(not(feature = "profile-fortress"))]
    pub fn set_epoll_hot(&self, fd: i32, ptr: *mut u8) {
        self.epoll_hot_fd.store(fd, Ordering::Relaxed);
        self.epoll_hot_ptr.store(ptr, Ordering::Relaxed);
    }

    /// Read the cached file fd number (-1 = no cache).
    #[cfg(not(feature = "profile-fortress"))]
    pub fn file_hot_fd(&self) -> i32 {
        self.file_hot_fd.load(Ordering::Relaxed)
    }

    /// Invalidate all hot-fd caches (epoll + file) if `fd` matches.
    #[cfg(not(feature = "profile-fortress"))]
    pub fn invalidate_hot_fd(&self, fd: i32) {
        if self.epoll_hot_fd.load(Ordering::Relaxed) == fd {
            self.epoll_hot_fd.store(-1, Ordering::Relaxed);
            self.epoll_hot_ptr.store(core::ptr::null_mut(), Ordering::Relaxed);
        }
        if self.file_hot_fd.load(Ordering::Relaxed) == fd {
            self.file_hot_fd.store(-1, Ordering::Relaxed);
            self.file_hot_ptr.store(core::ptr::null_mut(), Ordering::Relaxed);
        }
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

    /// Save the old signal mask for restoration by rt_sigreturn after sigsuspend.
    pub fn sigsuspend_save_mask(&self, mask: SigSet) {
        self.sigsuspend_saved_mask.store(mask.bits(), Ordering::Relaxed);
        self.sigsuspend_has_mask.store(true, Ordering::Release);
    }

    /// If a sigsuspend mask was saved, restore it and return true.
    pub fn sigsuspend_restore_mask(&self) -> bool {
        if self.sigsuspend_has_mask.swap(false, Ordering::Acquire) {
            let bits = self.sigsuspend_saved_mask.load(Ordering::Relaxed);
            self.sigset_store(SigSet::from_raw(bits));
            true
        } else {
            false
        }
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
            | ProcessState::ExitedWith(_)
            | ProcessState::ExitedBySignal(_) => {
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
        if matches!(old_state, ProcessState::ExitedWith(_) | ProcessState::ExitedBySignal(_)) {
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

    /// Resumes a process with scheduling priority boost (front of queue).
    /// Used for timer-based wakeups (nanosleep, sleep_ms) — the process
    /// has been sleeping and deserves prompt scheduling over CPU-bound
    /// threads.  Regular resume() enqueues at back (for poll/signal wakes
    /// which should not starve preempted threads).
    pub fn resume_boosted(&self) {
        let old_state = self.state.swap(ProcessState::Runnable);
        if matches!(old_state, ProcessState::ExitedWith(_) | ProcessState::ExitedBySignal(_)) {
            self.state.store(old_state);
            return;
        }
        if old_state == ProcessState::Runnable {
            return;
        }
        if kevlar_platform::arch::interrupts_enabled() {
            while !self.arch.context_saved.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        }
        SCHEDULER.lock().enqueue_front(self.pid);
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

    /// Call `f` with a borrowed reference to the opened file, avoiding an Arc
    /// clone on the unshared-fd-table fast path.
    ///
    /// `f` must not call back into the fd table (no close, no open).
    #[inline]
    pub fn with_file<F, R>(&self, fd: Fd, f: F) -> Result<R>
    where
        F: FnOnce(&OpenedFile) -> Result<R>,
    {
        #[cfg(not(feature = "profile-fortress"))]
        if Arc::strong_count(&self.opened_files) == 1 {
            // Hot-fd cache: skip fd table lookup on repeat calls to the same fd.
            // Safety: strong_count == 1 proves single-owner fd table access.
            // The cached pointer is into an Arc<OpenedFile> held by the fd table.
            // Invalidated by close/dup2/dup3/close_range before the Arc is dropped.
            let fd_int = fd.as_int();
            if fd_int == self.file_hot_fd.load(Ordering::Relaxed) {
                let ptr = self.file_hot_ptr.load(Ordering::Relaxed);
                if !ptr.is_null() {
                    #[allow(unsafe_code)]
                    let opened_file = unsafe { &*(ptr as *const OpenedFile) };
                    return f(opened_file);
                }
            }
            #[allow(unsafe_code)]
            let table = unsafe { self.opened_files.get_unchecked() };
            let opened_file = table.get(fd)?;
            // Populate cache: store raw pointer into the Arc<OpenedFile>.
            let ptr: *const OpenedFile = &**opened_file;
            self.file_hot_fd.store(fd_int, Ordering::Relaxed);
            self.file_hot_ptr.store(ptr as *mut u8, Ordering::Relaxed);
            return f(opened_file);
        }
        let file = self.opened_files.lock_no_irq().get(fd)?.clone();
        f(&file)
    }

    /// Terminates the **current** process.
    pub fn exit(status: c_int) -> ! {
        // Process::exit diverges — the SpanGuard's Drop never runs, so record
        // the exit span manually at the "point of no return" below (just
        // before `switch()` that doesn't return here).
        let _exit_start = debug::tracer::span_enter();
        let current = current_process();
        if current.pid == PId::new(1) {
            // Dump syscall profile before halting (if profiling was enabled).
            if debug::profiler::is_enabled() {
                debug::profiler::dump_syscall_profile(
                    crate::syscalls::syscall_name_by_number,
                );
            }
            if debug::tracer::is_enabled() {
                debug::tracer::dump_span_profile();
            }
            if debug::htrace::is_enabled() {
                debug::htrace::dump_all_cpus();
            }
            // Always dump htrace on non-zero exit — helps diagnose test failures.
            if status != 0 && !debug::htrace::is_enabled() {
                debug::htrace::dump_all_cpus();
            }
            // ktrace: dump binary trace via debugcon on PID 1 exit.
            #[cfg(feature = "ktrace")]
            if debug::ktrace::is_enabled() {
                debug::ktrace::dump_mm_events();
                debug::ktrace::dump_summary();
                debug::ktrace::dump();
            }
            // Dump PID 1 syscall trace for debugging.
            warn!("PID 1 exiting with status {}", status);
            crate::syscalls::dump_pid1_trace();
            info!("init exited with status {}, halting system", status);

            // Kill all remaining processes before halting to prevent GPF from
            // orphaned processes running with stale page tables after halt.
            //
            // CRITICAL: snapshot every Arc<Process> we want to signal BEFORE
            // calling send_signal, because send_signal → resume → SCHEDULER.lock
            // and SCHEDULER (rank 30) sits BELOW PROCESSES (rank 40) in the
            // lockdep order.  The previous version used
            //   if let Some(proc) = PROCESSES.lock().get(&pid).cloned()
            // which keeps the PROCESSES guard alive until the end of the
            // `if let` body — meaning send_signal runs while we still hold
            // PROCESSES, violating the order and panicking lockdep on PID 1
            // exit (e.g. when test-xfce halts the system at the end of the
            // session-component check).  Two-phase: collect, then signal.
            {
                let to_kill: Vec<Arc<Process>> = {
                    let table = PROCESSES.lock();
                    table.iter()
                        .filter(|(p, _)| **p != PId::new(1))
                        .map(|(_, proc)| proc.clone())
                        .collect()
                };
                for proc in to_kill {
                    proc.send_signal(SIGKILL);
                }
            }

            kevlar_platform::arch::halt();
        }

        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessExit {
            pid: current.pid().as_i32(),
            status,
            by_signal: false,
        });


        // Remove from cgroup member list so dead PIDs don't accumulate.
        {
            let cg = current.cgroup();
            cg.member_pids.lock().retain(|p| *p != current.pid);
        }

        // Release all POSIX record locks held by this process.
        crate::syscalls::fcntl::release_all_record_locks(current.pid().as_i32());

        // Preserve ExitedBySignal if already set (from exit_by_signal).
        if !matches!(current.state(), ProcessState::ExitedBySignal(_)) {
            current.set_state(ProcessState::ExitedWith(status));
        }

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

        // Ghost-fork: restore parent's writable PTEs using the child's
        // address list (O(writable_pages) instead of O(all_PTEs)).
        if current.vfork_parent.is_some() {
            if let Some(parent) = current.parent.upgrade() {
                if let Some(vm_ref) = parent.vm().as_ref() {
                    let cow_addrs: Vec<usize> = current.vm().as_ref()
                        .map(|v| v.lock().ghost_cow_addrs.clone())
                        .unwrap_or_default();
                    vm_ref.lock().restore_writable_with_list(&cow_addrs);
                }
            }
        }

        // Ghost-fork / vfork: set done flag BEFORE sending SIGCHLD.
        current.ghost_fork_done.store(true, Ordering::Release);
        current.wake_vfork_parent();

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
            // Always wake waiters so wait4/waitpid sees the new exit state.
            // This is needed even when SIGCHLD is Ignore (the default) —
            // send_signal skips Ignore-disposition signals, but wait4 must
            // still see the child's exit.
            JOIN_WAIT_QUEUE.wake_all();

            // Close opened files here instead of in Drop::drop because `proc`
            // is not dropped until it's joined by the parent process. Drop them
            // to make pipes closed.
            current.opened_files.lock_no_irq().close_all();
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
        // Exit path complete up to this point — record the span now, since
        // `switch()` below never returns here so any SpanGuard's Drop wouldn't
        // fire.
        debug::tracer::span_exit(debug::tracer::span::EXIT_TOTAL, _exit_start);
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
        // Re-enable interrupts.  Fault handlers may enter via interrupt gates
        // (which clear IF).  Running the entire exit path with IF=0 means
        // SpinLock save/restore cycles never re-enable interrupts — every lock
        // acquire/release just restores IF=0.  If two processes crash on two
        // CPUs simultaneously, both exit paths run with IF=0 permanently, and
        // any spinlock contention becomes unbounded (no timer can break it).
        // Enabling interrupts here — before any lock is touched — matches
        // Linux's cond_local_irq_enable() in its trap handlers.
        kevlar_platform::arch::enable_interrupts();

        let current = current_process();
        let pid = current.pid().as_i32();
        let cmdline = current.cmdline();
        warn!("PID {} ({}) killed by signal {}", pid, cmdline.as_str(), signal);
        if pid == 1 {
            crate::syscalls::dump_pid1_trace();
        }

        // ── Crash report (lightweight) ────────────────────────────────
        // Skip VMA listing to avoid VM lock deadlock: if a sibling thread
        // needs the VM lock (e.g. page fault), the crash dump would hold it
        // forever on single CPU (preempted but never rescheduled).
        debug::emit(DebugFilter::PROCESS, &DebugEvent::ProcessExit {
            pid,
            status: 128 + signal,
            by_signal: true,
        });
        drop(cmdline);
        // Set signal-death state BEFORE exit() so wait4 sees WIFSIGNALED.
        current_process().set_state(ProcessState::ExitedBySignal(signal as c_int));
        Process::exit(signal as c_int);
    }

    /// Sends a signal.
    pub fn send_signal(&self, signal: Signal) {
        crate::debug::htrace::enter(
            crate::debug::htrace::id::SIGNAL_SEND,
            ((self.pid().as_i32() as u32) << 8) | (signal as u32),
        );

        // TASK #26 DIAG: log every SIGKILL delivery with sender info.
        // Used to find the source of the dbus-daemon SIGKILL during
        // XFCE startup — when this prints "from=N to=M" we can trace
        // the sender's PID back to a userspace process or a kernel
        // fault-handler path.
        if signal == SIGKILL {
            let sender_pid = crate::process::try_current_pid();
            let sender_name = if sender_pid > 0 {
                // Best-effort cmdline of sender.
                if let Some(p) = Process::find_by_pid(crate::process::PId::new(sender_pid)) {
                    p.cmdline().as_str().chars().take(48).collect::<alloc::string::String>()
                } else {
                    alloc::string::String::from("?")
                }
            } else {
                alloc::string::String::from("kernel")
            };
            let target_name = self.cmdline().as_str().chars().take(48).collect::<alloc::string::String>();
            warn!(
                "SIGKILL: from pid={} ({:?}) to pid={} ({:?})",
                sender_pid, sender_name, self.pid().as_i32(), target_name,
            );
        }

        // SIGCONT always continues a stopped process, even if SIGCONT is blocked.
        if signal == SIGCONT {
            self.continue_process();
        }

        let mut sigs = self.signals.lock_no_irq();
        let action = sigs.get_action(signal);

        // Signals with Ignore disposition (default SIGCHLD, SIGURG, SIGWINCH)
        // should NOT be queued or interrupt sleep — matching POSIX/Linux behavior.
        // If the user installs a handler (SigAction::Handler), we do queue it.
        // SIGKILL and SIGSTOP can NEVER be ignored (POSIX).
        if matches!(action, SigAction::Ignore) && signal != SIGKILL && signal != SIGSTOP {
            crate::debug::htrace::exit(crate::debug::htrace::id::SIGNAL_SEND, 0);
            return;
        }

        sigs.signal(signal);
        drop(sigs);

        self.signal_pending.fetch_or(1 << (signal - 1), Ordering::Release);

        #[cfg(feature = "ktrace-mm")]
        {
            let pending_after = self.signal_pending.load(Ordering::Relaxed);
            let self_addr = self as *const _ as usize;
            crate::debug::ktrace::trace(
                crate::debug::ktrace::event::SIGNAL_SEND,
                self.pid().as_i32() as u32, signal as u32,
                pending_after, self_addr as u32, (self_addr >> 32) as u32,
            );
        }

        self.resume();

        // Wake poll/epoll waiters so signalfd can detect the new signal.
        crate::poll::POLL_WAIT_QUEUE.wake_all();
        crate::debug::htrace::exit(crate::debug::htrace::id::SIGNAL_SEND, signal as u32);
    }

    /// Returns true if we're inside a signal handler (the signaled frame
    /// stack has saved contexts). Used to detect nested faults — if SIGSEGV
    /// fires while already handling a signal, the process must be killed.
    pub fn is_in_signal_handler(&self) -> bool {
        self.signaled_frame_stack.lock_no_irq()
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    }

    /// Returns `true` if there's a deliverable (pending AND unblocked) signal.
    /// SIGKILL and SIGSTOP can never be blocked (POSIX), so they are always
    /// deliverable regardless of the signal mask.
    pub fn has_pending_signals(&self) -> bool {
        let pending = self.signal_pending.load(Ordering::Relaxed);
        let mut blocked = self.sigset_load().bits() as u32;
        // SIGKILL (9) and SIGSTOP (19) can NEVER be blocked (POSIX).
        blocked &= !((1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)));
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
            let mut sigs = current.signals.lock_no_irq();
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
                        Process::exit_by_signal(signal);
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
                    SigAction::Handler { handler, restorer, on_altstack, sa_mask } => {
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
                        #[cfg(target_arch = "x86_64")]
                        let original_rsp = { frame.rsp };
                        #[cfg(target_arch = "aarch64")]
                        let original_rsp = { frame.sp };
                        {
                            let mut guard = current.signaled_frame_stack.lock_no_irq();
                            let stack = guard.get_or_insert_with(
                                || alloc::boxed::Box::new(arrayvec::ArrayVec::new()));
                            if !stack.is_full() {
                                stack.push(*frame);
                            }
                        }

                        // Save the current mask so sigreturn can restore it.
                        let old_mask = current.sigset_load();

                        // Switch to alternate signal stack if SA_ONSTACK and alt stack registered.
                        if on_altstack {
                            let alt_sp = current.alt_stack_sp.load(core::sync::atomic::Ordering::Relaxed);
                            let alt_size = current.alt_stack_size.load(core::sync::atomic::Ordering::Relaxed);
                            if alt_sp != 0 && alt_size > 0 {
                                let alt_top = alt_sp + alt_size;
                                #[cfg(target_arch = "x86_64")]
                                {
                                    let sp = frame.rsp as usize;
                                    if sp < alt_sp || sp >= alt_top {
                                        frame.rsp = alt_top as u64;
                                    }
                                }
                                #[cfg(target_arch = "aarch64")]
                                {
                                    let sp = frame.sp as usize;
                                    if sp < alt_sp || sp >= alt_top {
                                        frame.sp = alt_top as u64;
                                    }
                                }
                            }
                        }

                        // Set usercopy context for fault attribution.
                        debug::usercopy::set_context("signal_stack_setup");
                        let result = current.arch.setup_signal_stack(frame, signal, handler, restorer, old_mask.bits(), original_rsp);
                        debug::usercopy::clear_context();


                        // Store ctx_base for sigreturn (needed for alt stack).
                        if let Ok(ctx_base) = &result {
                            let mut guard = current.signal_ctx_base_stack.lock_no_irq();
                            let cbs = guard.get_or_insert_with(
                                || alloc::boxed::Box::new(arrayvec::ArrayVec::new()));
                            if !cbs.is_full() {
                                cbs.push(*ctx_base);
                            }
                        }

                        // ktrace: log frame state after signal setup.
                        #[cfg(feature = "ktrace-mm")]
                        {
                            let deliver_pc = {
                                #[cfg(target_arch = "aarch64")] { frame.pc as u32 }
                                #[cfg(target_arch = "x86_64")] { 0u32 }
                            };
                            let deliver_lr = {
                                #[cfg(target_arch = "aarch64")] { frame.regs[30] as u32 }
                                #[cfg(target_arch = "x86_64")] { 0u32 }
                            };
                            crate::debug::ktrace::trace(
                                crate::debug::ktrace::event::SIGNAL_DELIVER,
                                signal as u32,
                                frame.regs[0] as u32,
                                deliver_pc,
                                deliver_lr,
                                handler.value() as u32,
                            );
                        }

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
        let popped = current.signaled_frame_stack.lock_no_irq()
            .as_mut().and_then(|s| s.pop());
        let ctx_base = current.signal_ctx_base_stack.lock_no_irq()
            .as_mut().and_then(|s| s.pop()).unwrap_or(0);
        if let Some(signaled_frame) = popped {
            let saved_mask = current
                .arch
                .setup_sigreturn_stack(current_frame, &signaled_frame, ctx_base);
            current.sigset_store(SigSet::from_raw(saved_mask));
        } else {
            // No saved frame — spurious sigreturn. Kill the process.
            warn!("pid={}: spurious sigreturn with no saved frame — killing",
                  current.pid().as_i32());
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
        let _exec_span = debug::tracer::span_guard(debug::tracer::span::EXEC_TOTAL);
        let current = current_process();
        {
            let _g = debug::tracer::span_guard(debug::tracer::span::EXEC_CLOSE_CLOEXEC);
            // Invalidate hot-fd cache before closing CLOEXEC files —
            // the cached pointer may point to a file about to be dropped.
            #[cfg(not(feature = "profile-fortress"))]
            {
                current.epoll_hot_fd.store(-1, Ordering::Relaxed);
                current.epoll_hot_ptr.store(core::ptr::null_mut(), Ordering::Relaxed);
                current.file_hot_fd.store(-1, Ordering::Relaxed);
                current.file_hot_ptr.store(core::ptr::null_mut(), Ordering::Relaxed);
            }
            current.opened_files.lock().close_cloexec_files();
        }
        current.cmdline.borrow_mut().set_by_argv(argv);
        current.set_environ(envp);

        // Store resolved executable path for /proc/[pid]/exe.
        {
            let resolved = executable_path.resolve_absolute_path();
            let mut ep = current.exe_path.lock_no_irq();
            ep.clear();
            let _ = ep.try_push_str(resolved.as_str());
        }

        let root_fs = current.root_fs();
        let mut entry = {
            let _g = debug::tracer::span_guard(debug::tracer::span::EXEC_SETUP_USERSPACE);
            setup_userspace(executable_path, argv, envp, &root_fs)?
        };
        // Remap vDSO with this process's per-process page.
        #[cfg(target_arch = "x86_64")]
        {
            let vdso_paddr_val = current.vdso_data_paddr.load(Ordering::Relaxed);
            if vdso_paddr_val != 0 {
                let vdso_uaddr = UserVAddr::new(kevlar_platform::arch::vdso::VDSO_VADDR).unwrap();
                entry.vm.page_table_mut().map_user_page_with_prot(
                    vdso_uaddr,
                    kevlar_platform::address::PAddr::new(vdso_paddr_val as usize),
                    5,
                );
            }
        }

        // de_thread: per POSIX, execve terminates all other threads in the
        // thread group.  Kill siblings NOW — after setup_userspace succeeds
        // (point of no return) but BEFORE replacing the address space.
        //
        // Each sibling's Arc<SpinLock<VirtualMemory>> keeps the OLD page table
        // alive until gc_exited_processes() drops the Arc, so there is no
        // use-after-free even if a sibling is still executing on another CPU
        // for a few hundred nanoseconds after we mark it ExitedWith.
        {
            let _g = debug::tracer::span_guard(debug::tracer::span::EXEC_DE_THREAD);
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
        {
            let _g = debug::tracer::span_guard(debug::tracer::span::EXEC_SIGNAL_RESET);
            current.signals.lock_no_irq().reset_on_exec();
            if let Some(s) = current.signaled_frame_stack.lock_no_irq().as_mut() {
                s.clear();
            }
        }

        // Ghost-fork: extract cow_addrs from the OLD VM before replacing it.
        let ghost_cow_addrs: Option<Vec<usize>> = if current.vfork_parent.is_some() {
            let vm_ref = current.vm();
            vm_ref.as_ref().and_then(|arc| {
                let vm = arc.lock();
                if vm.is_ghost_forked {
                    Some(vm.ghost_cow_addrs.clone())
                } else {
                    None
                }
            })
        } else {
            None
        };

        entry.vm.page_table().switch();
        // Replace the Vm, then drop the old one *outside* the borrow_mut
        // scope.  Vm::Drop can take milliseconds (synchronous QSC grace
        // period, commit 253821f), and during that time any other CPU
        // that does `current.vm()` would observe a held mutable borrow
        // and panic "already mutably borrowed".  Using mem::replace
        // limits the borrow_mut to the swap itself.
        let _old_vm = core::mem::replace(
            &mut *current.vm.borrow_mut(),
            Some(Arc::new(SpinLock::new(entry.vm))),
        );
        // `_old_vm` (an Option<Arc<SpinLock<Vm>>>) is dropped here with
        // no vm borrow held.  The Arc may not be the last reference —
        // Vm::Drop only fires when refcount hits zero.

        // Ghost-fork: restore parent's writable PTEs using the saved list.
        if let Some(ref addrs) = ghost_cow_addrs {
            if let Some(parent) = current.parent.upgrade() {
                if let Some(vm_ref) = parent.vm().as_ref() {
                    vm_ref.lock().restore_writable_with_list(addrs);
                }
            }
        }

        // Ghost-fork / vfork: wake blocked parent now that exec has replaced
        // the address space. The parent's VM is no longer shared.
        current.ghost_fork_done.store(true, Ordering::Release);
        current.wake_vfork_parent();

        current
            .arch
            .setup_execve_stack(frame, entry.ip, entry.user_sp);

        Ok(())
    }

    /// Creates a new process. The calling process (`self`) will be the parent
    /// process of the created process. Returns the created child process.
    pub fn fork(parent: &Arc<Process>, parent_frame: &PtRegs) -> Result<Arc<Process>> {
        let _fork_span = debug::tracer::span_guard(debug::tracer::span::FORK_TOTAL);
        // Check cgroup pids.max limit before allocating resources.
        crate::cgroups::pids_controller::check_fork_allowed(&parent.cgroup())?;

        let parent_weak = Arc::downgrade(parent);
        let mut process_table = PROCESSES.lock();
        let pid = {
            let _s = debug::tracer::span_guard(debug::tracer::span::FORK_ALLOC_PID);
            alloc_pid(&mut process_table)?
        };
        let arch = {
            let _s = debug::tracer::span_guard(debug::tracer::span::FORK_ARCH);
            parent.arch.fork(parent_frame)
                .map_err(|_| crate::result::Error::new(Errno::ENOMEM))?
        };
        // Ghost-fork: duplicate page tables but skip refcount operations.
        // The parent is blocked until the child exec's or exits.
        // CoW faults in the child copy pages on demand (typically 2-3 pages
        // for musl's _Fork() wrapper). Saves ~8µs per fork.
        let ghost = GHOST_FORK_ENABLED.load(Ordering::Relaxed);
        let vm = if ghost {
            let _g = debug::tracer::span_guard(debug::tracer::span::FORK_GHOST);
            let forked = parent.vm().as_ref().unwrap().lock().ghost_fork()?;
            Some(Arc::new(SpinLock::new(forked)))
        } else {
            let _g = debug::tracer::span_guard(debug::tracer::span::FORK_PAGE_TABLE);
            let forked = parent.vm().as_ref().unwrap().lock().fork()?;
            Some(Arc::new(SpinLock::new(forked)))
        };
        let opened_files = {
            let _s = debug::tracer::span_guard(debug::tracer::span::FORK_FILES_CLONE);
            parent.opened_files().lock().clone()
        };
        let process_group = parent.process_group();
        let sig_set = parent.sigset_load();
        let parent_umask = parent.umask.load();
        let child_utsname = *parent.cached_utsname.lock_no_irq();
        #[cfg(target_arch = "x86_64")]
        let child_vdso = kevlar_platform::arch::vdso::alloc_process_page(
            pid.as_i32(), pid.as_i32(),
            parent.uid.load(Ordering::Relaxed),
            parent.nice.load(Ordering::Relaxed),
            &child_utsname,
        ).map(|p| p.value() as u64).unwrap_or(0);

        // Hoist per-field clones out of Arc::new so they can be measured
        // independently from the big Process-struct allocation.
        let (cloned_cmdline, cloned_environ, cloned_signals, cloned_comm,
             cloned_rootfs, cloned_exe_path, cloned_groups) = {
            let _s = debug::tracer::span_guard(debug::tracer::span::FORK_INNER_CLONES);
            (
                parent.cmdline().clone(),
                parent.environ.lock_no_irq().clone(),
                parent.signals.lock_no_irq().fork_clone(),
                parent.comm.lock_no_irq().clone(),
                parent.root_fs().lock().clone(),
                parent.exe_path.lock_no_irq().clone(),
                parent.groups(),
            )
        };

        let _fork_struct_span = debug::tracer::span_guard(debug::tracer::span::FORK_STRUCT);
        let child = Arc::new(Process {
            is_idle: false,
            process_group: AtomicRefCell::new(Arc::downgrade(&process_group)),
            pid,
            tgid: pid, // fork creates a new thread group; child becomes its own leader
            session_id: AtomicI32::new(parent.session_id()),
            state: AtomicProcessState::new(ProcessState::Runnable),
            parent: parent_weak,
            cmdline: AtomicRefCell::new(cloned_cmdline),
            environ: SpinLock::new(cloned_environ),
            children: SpinLock::new(Vec::new()),
            vm: AtomicRefCell::new(vm),
            opened_files: Arc::new(SpinLock::new(opened_files)),
            root_fs: AtomicRefCell::new(Arc::new(SpinLock::new(cloned_rootfs))),
            arch,
            signals: Arc::new(SpinLock::new(cloned_signals)),
            signal_pending: AtomicU32::new(0),
            signaled_frame_stack: SpinLock::new(None),
            signal_ctx_base_stack: SpinLock::new(None),
            sigset: AtomicU64::new(sig_set.bits()),
            umask: AtomicCell::new(parent_umask),
            uid: AtomicU32::new(parent.uid.load(Ordering::Relaxed)),
            euid: AtomicU32::new(parent.euid.load(Ordering::Relaxed)),
            suid: AtomicU32::new(parent.suid.load(Ordering::Relaxed)),
            gid: AtomicU32::new(parent.gid.load(Ordering::Relaxed)),
            egid: AtomicU32::new(parent.egid.load(Ordering::Relaxed)),
            sgid: AtomicU32::new(parent.sgid.load(Ordering::Relaxed)),
            nice: AtomicI32::new(parent.nice.load(Ordering::Relaxed)),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(cloned_comm),
            clear_child_tid: AtomicUsize::new(0), // POSIX: not inherited across fork
            vfork_parent: if ghost { Some(parent.pid()) } else { None },
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            groups: SpinLock::new(cloned_groups),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(pid.as_i32()),
            syscall_trace: SyscallTrace::new(),
            exe_path: SpinLock::new(cloned_exe_path),
            ghost_fork_done: AtomicBool::new(false),
            sigsuspend_saved_mask: AtomicU64::new(0),
            sigsuspend_has_mask: AtomicBool::new(false),
            alt_stack_sp: AtomicUsize::new(0),
            alt_stack_size: AtomicUsize::new(0),
            alt_stack_flags: AtomicU32::new(0),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            rlimits: AtomicRlimits::new(parent.rlimits()),
            cached_utsname: SpinLock::new(child_utsname),
            #[cfg(target_arch = "x86_64")]
            vdso_data_paddr: AtomicU64::new(child_vdso),
        });
        drop(_fork_struct_span);

        let _fork_reg_span = debug::tracer::span_guard(debug::tracer::span::FORK_REGISTER);
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

    /// Creates a child process that shares the parent's address space.
    /// The parent is suspended until the child calls _exit() or exec().
    /// No page table copy, no TLB flush — much faster than fork().
    pub fn vfork(parent: &Arc<Process>, parent_frame: &PtRegs) -> Result<Arc<Process>> {
        crate::cgroups::pids_controller::check_fork_allowed(&parent.cgroup())?;

        let parent_weak = Arc::downgrade(parent);
        let mut process_table = PROCESSES.lock();
        let pid = alloc_pid(&mut process_table)?;
        let arch = parent.arch.fork(parent_frame)
            .map_err(|_| crate::result::Error::new(Errno::ENOMEM))?;
        // Share the parent's VM — no page table copy!
        let vm = parent.vm().clone();
        let opened_files = parent.opened_files().lock().clone();
        let process_group = parent.process_group();
        let sig_set = parent.sigset_load();
        let child_utsname = *parent.cached_utsname.lock_no_irq();
        #[cfg(target_arch = "x86_64")]
        let child_vdso = kevlar_platform::arch::vdso::alloc_process_page(
            pid.as_i32(), pid.as_i32(),
            parent.uid.load(Ordering::Relaxed),
            parent.nice.load(Ordering::Relaxed),
            &child_utsname,
        ).map(|p| p.value() as u64).unwrap_or(0);

        let child = Arc::new(Process {
            is_idle: false,
            process_group: AtomicRefCell::new(Arc::downgrade(&process_group)),
            pid,
            tgid: pid,
            session_id: AtomicI32::new(parent.session_id()),
            state: AtomicProcessState::new(ProcessState::Runnable),
            parent: parent_weak,
            cmdline: AtomicRefCell::new(parent.cmdline().clone()),
            environ: SpinLock::new(parent.environ.lock_no_irq().clone()),
            children: SpinLock::new(Vec::new()),
            vm: AtomicRefCell::new(vm),
            opened_files: Arc::new(SpinLock::new(opened_files)),
            root_fs: AtomicRefCell::new({
                let parent_fs = parent.root_fs();
                let cloned = parent_fs.lock().clone();
                Arc::new(SpinLock::new(cloned))
            }),
            arch,
            signals: Arc::new(SpinLock::new(parent.signals.lock_no_irq().fork_clone())),
            signal_pending: AtomicU32::new(0),
            signaled_frame_stack: SpinLock::new(None),
            signal_ctx_base_stack: SpinLock::new(None),
            sigset: AtomicU64::new(sig_set.bits()),
            umask: AtomicCell::new(parent.umask.load()),
            uid: AtomicU32::new(parent.uid.load(Ordering::Relaxed)),
            euid: AtomicU32::new(parent.euid.load(Ordering::Relaxed)),
            suid: AtomicU32::new(parent.suid.load(Ordering::Relaxed)),
            gid: AtomicU32::new(parent.gid.load(Ordering::Relaxed)),
            egid: AtomicU32::new(parent.egid.load(Ordering::Relaxed)),
            sgid: AtomicU32::new(parent.sgid.load(Ordering::Relaxed)),
            nice: AtomicI32::new(parent.nice.load(Ordering::Relaxed)),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(parent.comm.lock_no_irq().clone()),
            clear_child_tid: AtomicUsize::new(0),
            vfork_parent: Some(parent.pid()),
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            stime: AtomicU64::new(0),
            groups: SpinLock::new(parent.groups()),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(pid.as_i32()),
            syscall_trace: SyscallTrace::new(),
            exe_path: SpinLock::new(parent.exe_path.lock_no_irq().clone()),
            ghost_fork_done: AtomicBool::new(false),
            sigsuspend_saved_mask: AtomicU64::new(0),
            sigsuspend_has_mask: AtomicBool::new(false),
            alt_stack_sp: AtomicUsize::new(0),
            alt_stack_size: AtomicUsize::new(0),
            alt_stack_flags: AtomicU32::new(0),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            rlimits: AtomicRlimits::new(parent.rlimits()),
            cached_utsname: SpinLock::new(child_utsname),
            #[cfg(target_arch = "x86_64")]
            vdso_data_paddr: AtomicU64::new(child_vdso),
        });

        let parent_cg = parent.cgroup();
        *child.cgroup.borrow_mut() = Some(parent_cg.clone());
        parent_cg.member_pids.lock().push(pid);

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
        SCHEDULER.lock().enqueue(pid);

        FORK_TOTAL.fetch_add(1, Ordering::Relaxed);
        Ok(child)
    }

    /// Wake the vfork parent (if any). Called from _exit and exec.
    pub fn wake_vfork_parent(&self) {
        if let Some(parent_pid) = self.vfork_parent {
            VFORK_WAIT_QUEUE.wake_all();
        }
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
        is_vfork: bool,      // CLONE_VFORK: set vfork_parent so exec/exit wakes parent
        is_thread: bool,     // CLONE_THREAD: share thread group (tgid)
    ) -> Result<Arc<Process>> {
        let mut process_table = PROCESSES.lock();
        let pid = alloc_pid(&mut process_table)?;

        let fs_base = if newtls != 0 { newtls } else { parent.arch.fsbase() };
        let arch = arch::Process::new_thread(frame, child_stack, fs_base)
            .map_err(|_| crate::result::Error::new(Errno::ENOMEM))?;

        let child = Arc::new(Process {
            is_idle: false,
            process_group: AtomicRefCell::new(parent.process_group.borrow().clone()),
            pid,
            tgid: if is_thread { parent.tgid } else { pid },
            session_id: AtomicI32::new(parent.session_id()),
            state: AtomicProcessState::new(ProcessState::Runnable),
            parent: Arc::downgrade(parent),
            cmdline: AtomicRefCell::new(parent.cmdline().clone()),
            environ: SpinLock::new(parent.environ.lock_no_irq().clone()),
            children: SpinLock::new(Vec::new()),
            // Share address space and signal handlers.
            vm: AtomicRefCell::new(parent.vm().as_ref().map(Arc::clone)),
            // CLONE_FILES: share fd table. Without it, child gets a copy
            // so exec's CLOEXEC doesn't destroy parent's pipe fds.
            opened_files: if is_thread {
                // CLONE_THREAD implies CLONE_FILES
                Arc::clone(&parent.opened_files)
            } else {
                Arc::new(SpinLock::new(parent.opened_files.lock_no_irq().clone()))
            },
            root_fs: AtomicRefCell::new(parent.root_fs()),
            signals: if is_thread {
                Arc::clone(&parent.signals)
            } else {
                Arc::new(SpinLock::new(parent.signals.lock_no_irq().fork_clone()))
            },
            signal_pending: AtomicU32::new(0),
            signaled_frame_stack: SpinLock::new(None),
            signal_ctx_base_stack: SpinLock::new(None),
            sigset: AtomicU64::new(parent.sigset_load().bits()),
            umask: AtomicCell::new(parent.umask.load()),
            uid: AtomicU32::new(parent.uid.load(Ordering::Relaxed)),
            euid: AtomicU32::new(parent.euid.load(Ordering::Relaxed)),
            suid: AtomicU32::new(parent.suid.load(Ordering::Relaxed)),
            gid: AtomicU32::new(parent.gid.load(Ordering::Relaxed)),
            egid: AtomicU32::new(parent.egid.load(Ordering::Relaxed)),
            sgid: AtomicU32::new(parent.sgid.load(Ordering::Relaxed)),
            nice: AtomicI32::new(parent.nice.load(Ordering::Relaxed)),
            is_child_subreaper: AtomicBool::new(false),
            comm: SpinLock::new(parent.comm.lock_no_irq().clone()),
            clear_child_tid: AtomicUsize::new(0),
            vfork_parent: if is_vfork { Some(parent.pid()) } else { None },
            start_ticks: crate::timer::monotonic_ticks() as u64,
            utime: AtomicU64::new(0),
            groups: SpinLock::new(parent.groups()),
            stime: AtomicU64::new(0),
            cgroup: AtomicRefCell::new(None),
            namespaces: AtomicRefCell::new(None),
            ns_pid: AtomicI32::new(pid.as_i32()),
            syscall_trace: SyscallTrace::new(),
            exe_path: SpinLock::new(parent.exe_path.lock_no_irq().clone()),
            ghost_fork_done: AtomicBool::new(false),
            sigsuspend_saved_mask: AtomicU64::new(0),
            sigsuspend_has_mask: AtomicBool::new(false),
            alt_stack_sp: AtomicUsize::new(0),
            alt_stack_size: AtomicUsize::new(0),
            alt_stack_flags: AtomicU32::new(0),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            epoll_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_fd: AtomicI32::new(-1),
            #[cfg(not(feature = "profile-fortress"))]
            file_hot_ptr: AtomicPtr::new(core::ptr::null_mut()),
            rlimits: AtomicRlimits::new(parent.rlimits()),
            cached_utsname: SpinLock::new(*parent.cached_utsname.lock_no_irq()),
            #[cfg(target_arch = "x86_64")]
            vdso_data_paddr: AtomicU64::new({
                // Threads share the parent's vDSO page. Set tid=0 so
                // __vdso_gettid falls back to syscall in multi-threaded processes.
                let p = parent.vdso_data_paddr.load(Ordering::Relaxed);
                if p != 0 {
                    kevlar_platform::arch::vdso::update_tid(
                        kevlar_platform::address::PAddr::new(p as usize), 0,
                    );
                }
                p
            }),
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
        parent.children().push(child.clone());
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

        // Free the per-process vDSO data page (allocated in fork/vfork).
        // The vDSO is mapped at a FIXED user VA (VDSO_VADDR = 0x1000_0000_0000)
        // in every process. Without an all-CPU TLB invalidate before freeing,
        // a CPU that recently ran this process retains a stale VDSO_VADDR →
        // vdso_paddr translation. The freed page is reissued by the buddy
        // allocator (often as a kernel stack), and any subsequent user write
        // through the stale VA corrupts the new owner.
        //
        // We can't send an IPI from Drop with IF=0 (it would deadlock against
        // TLB_SHOOTDOWN_LOCK), so just bump the global PCID generation. Every
        // CPU does a full CR3 reload on its next context switch, which
        // invalidates all entries for the old PCID. This is safe in
        // either IF=1 or IF=0 contexts.
        #[cfg(target_arch = "x86_64")]
        {
            let vdso_paddr = self.vdso_data_paddr.load(core::sync::atomic::Ordering::Relaxed);
            if vdso_paddr != 0 {
                kevlar_platform::arch::bump_global_pcid_generation();
                kevlar_platform::page_allocator::free_pages(
                    kevlar_platform::address::PAddr::new(vdso_paddr as usize),
                    1,
                );
            }
        }

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
    // interpreter.  Use the ORIGINAL argv[0] (chroot-relative) rather
    // than resolve_absolute_path() which returns the host-absolute path
    // and breaks scripts inside chroots.
    argv.push(script_argv[0]);

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

    // Update exe_path to the interpreter (e.g. /bin/sh), not the script.
    // Linux /proc/self/exe always points to the loaded ELF binary.
    // BusyBox relies on this to re-exec itself as an applet via /proc/self/exe.
    {
        let resolved = shebang_path.resolve_absolute_path();
        let current = crate::process::current_process();
        let mut ep = current.exe_path.lock_no_irq();
        ep.clear();
        let _ = ep.try_push_str(resolved.as_str());
    }

    do_setup_userspace(shebang_path, &argv, envp, root_fs, false)
}

/// Convert ELF p_flags to MMapProt.  The bit positions differ:
///   PF_X = 1  →  PROT_EXEC = 4
///   PF_W = 2  →  PROT_WRITE = 2
///   PF_R = 4  →  PROT_READ = 1
fn elf_flags_to_prot(p_flags: u32) -> MMapProt {
    let mut prot = MMapProt::empty();
    if p_flags & 4 != 0 { prot |= MMapProt::PROT_READ; }   // PF_R
    if p_flags & 2 != 0 { prot |= MMapProt::PROT_WRITE; }  // PF_W
    if p_flags & 1 != 0 { prot |= MMapProt::PROT_EXEC; }   // PF_X
    prot
}

// --- VMA template cache ---
// Caches the VMA layout produced by load_elf_segments for frequently exec'd
// binaries (e.g. BusyBox). On cache hit, skips ELF segment iteration and
// gap filling, directly adding the cached VMA entries to the new VM.
// Only used for static executables (base_offset=0, same addresses every time).

struct VmaTemplateEntry {
    start: usize,
    len: usize,
    prot_bits: i32,
    is_file: bool,
    file_offset: usize,
    file_size: usize,
}

static VMA_TEMPLATE_CACHE: SpinLock<Option<hashbrown::HashMap<usize, Vec<VmaTemplateEntry>>>> =
    SpinLock::new(None);

fn vma_template_lookup(file_ptr: usize) -> Option<Vec<VmaTemplateEntry>> {
    let cache = VMA_TEMPLATE_CACHE.lock_no_irq();
    cache.as_ref().and_then(|map| map.get(&file_ptr).map(|v| {
        v.iter().map(|e| VmaTemplateEntry {
            start: e.start,
            len: e.len,
            prot_bits: e.prot_bits,
            is_file: e.is_file,
            file_offset: e.file_offset,
            file_size: e.file_size,
        }).collect()
    }))
}

fn save_vma_template(file_ptr: usize, vm: &Vm, vma_start_idx: usize) {
    let entries: Vec<VmaTemplateEntry> = vm.vm_areas()[vma_start_idx..]
        .iter()
        .map(|vma| {
            let (is_file, file_offset, file_size) = match vma.area_type() {
                VmAreaType::File { offset, file_size, .. } => (true, *offset, *file_size),
                VmAreaType::Anonymous | VmAreaType::DeviceMemory { .. } => (false, 0, 0),
            };
            VmaTemplateEntry {
                start: vma.start().value(),
                len: vma.end().value() - vma.start().value(),
                prot_bits: vma.prot().bits(),
                is_file,
                file_offset,
                file_size,
            }
        })
        .collect();
    let mut cache = VMA_TEMPLATE_CACHE.lock_no_irq();
    let map = cache.get_or_insert_with(hashbrown::HashMap::new);
    map.insert(file_ptr, entries);
}

fn apply_vma_template(
    vm: &mut Vm,
    template: &[VmaTemplateEntry],
    file: &Arc<dyn FileLike>,
) -> Result<()> {
    for entry in template {
        let area_type = if entry.is_file {
            VmAreaType::File {
                file: file.clone(),
                offset: entry.file_offset,
                file_size: entry.file_size,
            }
        } else {
            VmAreaType::Anonymous
        };
        vm.add_vm_area_with_prot(
            UserVAddr::new_nonnull(entry.start)?,
            entry.len,
            area_type,
            MMapProt::from_bits_truncate(entry.prot_bits),
            false,
        )?;
    }
    Ok(())
}

// --- Prefault template cache (Experiment 2) ---
// Caches the actual (vaddr, paddr, prot_flags) mappings produced by
// prefault_cached_pages for a given binary. On cache hit, replays the
// mappings directly (skip HashMap lookups, VMA iteration, huge page assembly).

use kevlar_platform::address::PAddr;

struct PrefaultTemplate {
    entries: Vec<(usize, PAddr, i32)>,
    huge_entries: Vec<(usize, PAddr, i32)>,
    /// PAGE_CACHE generation when this template was built.
    /// If PAGE_CACHE_GEN has advanced past this, the template is stale
    /// and we must fall through to prefault_cached_pages.
    cache_gen: u64,
}

static PREFAULT_TEMPLATE_CACHE: SpinLock<Option<hashbrown::HashMap<usize, PrefaultTemplate>>> =
    SpinLock::new(None);

fn prefault_template_lookup(file_ptr: usize) -> Option<u64> {
    let cache = PREFAULT_TEMPLATE_CACHE.lock_no_irq();
    cache.as_ref().and_then(|map| map.get(&file_ptr).map(|t| t.cache_gen))
}

fn apply_prefault_template(vm: &mut Vm, file_ptr: usize) {
    use kevlar_utils::alignment::align_down;
    use kevlar_platform::arch::HUGE_PAGE_SIZE;

    let cache = PREFAULT_TEMPLATE_CACHE.lock_no_irq();
    let template = match cache.as_ref().and_then(|map| map.get(&file_ptr)) {
        Some(t) => t,
        None => return,
    };

    // Map huge pages first.
    for &(vaddr, paddr, flags) in &template.huge_entries {
        if let Ok(uaddr) = UserVAddr::new_nonnull(vaddr) {
            if vm.page_table().is_pde_empty(uaddr) {
                kevlar_platform::page_refcount::page_ref_inc_huge(paddr);
                vm.page_table_mut().map_huge_user_page(uaddr, paddr, flags);
            }
        }
    }

    // Map 4KB pages, skipping those inside huge page regions.  Batch
    // contiguous same-prot entries through `batch_try_map_user_pages_with_prot`
    // to walk each leaf PT once instead of once per page — cuts ~4 µs of
    // per-iter overhead for busybox-sized binaries (~260 entries before
    // batching became a noticeable fraction of execve time).
    let entries = &template.entries;
    if entries.is_empty() { return; }

    let mut i = 0;
    // Reusable scratch for batch calls.  64 == batch primitive's per-call
    // max (u64 bitmap return).  For busybox's ~260 template entries,
    // widening from 32 to 64 halves the number of batch calls (leaf-PT
    // traversals + DSB/ISB fences).
    let mut batch_paddrs: [kevlar_platform::address::PAddr; 64] =
        [kevlar_platform::address::PAddr::new(0); 64];

    while i < entries.len() {
        let (vaddr_i, _, flags_i) = entries[i];
        // Skip entries inside huge-mapped regions (templated huge pages may
        // cover the 4K slots that follow).
        let huge_base_i = align_down(vaddr_i, HUGE_PAGE_SIZE);
        if let Ok(huge_uaddr) = UserVAddr::new_nonnull(huge_base_i) {
            if vm.page_table().is_huge_mapped(huge_uaddr).is_some() {
                i += 1;
                continue;
            }
        }

        // Find the end of a contiguous-VA, same-prot run starting at i.
        let mut run_end = i + 1;
        while run_end < entries.len() {
            let (v, _, f) = entries[run_end];
            let (v_prev, _, f_prev) = entries[run_end - 1];
            if v != v_prev + PAGE_SIZE || f != f_prev { break; }
            if run_end - i >= batch_paddrs.len() { break; }
            // Also stop the run at huge-page boundaries to preserve the
            // skip-inside-huge semantics (next iteration re-checks).
            if align_down(v, HUGE_PAGE_SIZE) != huge_base_i { break; }
            run_end += 1;
        }

        let run_len = run_end - i;
        if run_len == 1 {
            // Single-page fallback — same as before.
            let (_, paddr, _) = entries[i];
            if let Ok(uaddr) = UserVAddr::new_nonnull(vaddr_i) {
                kevlar_platform::page_refcount::page_ref_inc(paddr);
                if !vm.page_table_mut().try_map_user_page_with_prot(uaddr, paddr, flags_i) {
                    kevlar_platform::page_refcount::page_ref_dec(paddr);
                }
            }
            i = run_end;
            continue;
        }

        // Bump refcounts for the run up-front; undo for any that fail to
        // install (duplicate PTE already present).
        for k in 0..run_len {
            batch_paddrs[k] = entries[i + k].1;
            kevlar_platform::page_refcount::page_ref_inc(batch_paddrs[k]);
        }
        let start_uaddr = match UserVAddr::new_nonnull(vaddr_i) {
            Ok(u) => u,
            Err(_) => {
                for k in 0..run_len {
                    kevlar_platform::page_refcount::page_ref_dec(batch_paddrs[k]);
                }
                i = run_end;
                continue;
            }
        };
        let mapped = vm.page_table_mut().batch_try_map_user_pages_with_prot(
            start_uaddr, &batch_paddrs[..run_len], run_len, flags_i,
        );
        // Undo refcount for entries that weren't mapped.
        for k in 0..run_len {
            if mapped & (1u64 << k) == 0 {
                kevlar_platform::page_refcount::page_ref_dec(batch_paddrs[k]);
            }
        }
        i = run_end;
    }
}

fn build_and_save_prefault_template(vm: &Vm, file_ptr: usize) {
    use kevlar_utils::alignment::align_down;
    use kevlar_platform::arch::HUGE_PAGE_SIZE;

    let mut entries = Vec::new();
    let mut huge_entries = Vec::new();

    for vma in vm.vm_areas() {
        if let VmAreaType::File { file, .. } = vma.area_type() {
            if !file.is_content_immutable() { continue; }
            if Arc::as_ptr(file) as *const () as usize != file_ptr { continue; }

            let prot_flags = vma.prot().bits();
            let vma_start = vma.start().value();
            let vma_end = vma.end().value();

            let mut addr = align_up(vma_start, PAGE_SIZE);
            while addr + PAGE_SIZE <= vma_end {
                let uaddr = UserVAddr::new_nonnull(addr).unwrap();
                // Check for huge page mapping.
                let huge_base = align_down(addr, HUGE_PAGE_SIZE);
                if let Ok(huge_uaddr) = UserVAddr::new_nonnull(huge_base) {
                    if let Some(pde_val) = vm.page_table().is_huge_mapped(huge_uaddr) {
                        if !huge_entries.iter().any(|&(va, _, _): &(usize, PAddr, i32)| va == huge_base) {
                            let paddr = PAddr::new((pde_val & 0x000f_ffff_ffff_f000) as usize);
                            huge_entries.push((huge_base, paddr, 5));
                        }
                        addr = huge_base + HUGE_PAGE_SIZE;
                        continue;
                    }
                }
                if let Some(paddr) = vm.page_table().lookup_paddr(uaddr) {
                    entries.push((addr, paddr, prot_flags));
                }
                addr += PAGE_SIZE;
            }
        }
    }

    if entries.is_empty() && huge_entries.is_empty() {
        return;
    }

    let cache_gen = crate::mm::page_fault::PAGE_CACHE_GEN.load(core::sync::atomic::Ordering::Relaxed);
    let mut cache = PREFAULT_TEMPLATE_CACHE.lock_no_irq();
    let map = cache.get_or_insert_with(hashbrown::HashMap::new);
    map.insert(file_ptr, PrefaultTemplate { entries, huge_entries, cache_gen });
}

/// Load PT_LOAD segments from an ELF into the VM, then fill inter-segment
/// gaps with anonymous VMAs so that addresses within the full page-aligned
/// span are always backed by a VMA.  This is required because libc (e.g.
/// musl's `reclaim_gaps`) reuses these gap pages for its allocator.
/// Eagerly map writable data segment pages so the dynamic linker can apply
/// PIE relocations to ALL data pages before any fork. Without this, fork
/// children demand-fault raw file data with unpatched relocation pointers.
fn prefault_writable_segments(
    vm: &mut Vm,
    phdrs: &[ProgramHeader],
    base_offset: usize,
    file: &Arc<dyn FileLike>,
) -> Result<()> {
    use kevlar_platform::arch::PAGE_SIZE;
    use kevlar_utils::alignment::align_up;

    for phdr in phdrs {
        if phdr.p_type != PT_LOAD || phdr.p_flags & 2 == 0 || phdr.p_filesz == 0 {
            continue;
        }
        let seg_start = (phdr.p_vaddr as usize) + base_offset;
        let seg_file_offset = phdr.p_offset as usize;
        let seg_filesz = phdr.p_filesz as usize;
        let first_page = kevlar_utils::alignment::align_down(seg_start, PAGE_SIZE);
        let end_page = align_up(seg_start + seg_filesz, PAGE_SIZE);

        for page_addr in (first_page..end_page).step_by(PAGE_SIZE) {
            if let Ok(vaddr) = UserVAddr::new_nonnull(page_addr) {
                if vm.page_table().lookup_paddr(vaddr).is_some() {
                    continue;
                }
                // Only map pages that are within an actual VMA. Don't map
                // pages in the page-aligned padding beyond the segment end,
                // as alloc_vaddr_range may later assign those addresses to
                // different mmap'd files (e.g., shared libraries via dlopen).
                let in_vma = vm.vm_areas().iter().any(|a| a.contains(vaddr));
                if !in_vma {
                    continue;
                }
                let paddr = kevlar_platform::page_allocator::alloc_page(
                    kevlar_platform::page_allocator::AllocPageFlags::USER)?;
                let offset_in_page = if page_addr < seg_start { seg_start - page_addr } else { 0 };
                let page_file_start = if page_addr < seg_start { 0 } else { page_addr - seg_start };
                let file_off = seg_file_offset + page_file_start;
                let remaining = seg_filesz.saturating_sub(page_file_start);
                let copy_len = core::cmp::min(remaining, PAGE_SIZE - offset_in_page);
                if copy_len > 0 {
                    #[cfg(not(feature = "profile-fortress"))]
                    {
                        let buf = kevlar_platform::page_ops::page_as_slice_mut(paddr);
                        let _ = file.read(file_off,
                            (&mut buf[offset_in_page..offset_in_page + copy_len]).into(),
                            &kevlar_vfs::inode::OpenOptions::readwrite());
                    }
                }
                vm.page_table_mut().map_user_page_with_prot(vaddr, paddr,
                    elf_flags_to_prot(phdr.p_flags).bits());
                kevlar_platform::page_refcount::page_ref_init(paddr);
            }
        }
    }
    Ok(())
}

fn load_elf_segments(
    vm: &mut Vm,
    phdrs: &[ProgramHeader],
    base_offset: usize,
    file: &Arc<dyn FileLike>,
) -> Result<()> {
    use kevlar_utils::alignment::align_down;

    // Add VMAs for each PT_LOAD segment.  VMAs MUST be page-aligned:
    // the page fault handler looks up VMAs by page address, and if a VMA
    // only covers bytes 0x400000-0x400200 (512 bytes), accessing byte
    // 0x400201 has no VMA → SIGSEGV.  The VMA must cover the full pages
    // that the segment touches.  The file offset is adjusted to match
    // the page-aligned start.
    let mut page_ranges: Vec<(usize, usize)> = Vec::new();
    for phdr in phdrs {
        if phdr.p_type != PT_LOAD {
            continue;
        }

        let seg_start = (phdr.p_vaddr as usize) + base_offset;
        let seg_end = seg_start + phdr.p_memsz as usize;

        // Page-align: start rounds down, end rounds up.
        let page_start = align_down(seg_start, PAGE_SIZE);
        let page_end = align_up(seg_end, PAGE_SIZE);
        let page_len = page_end - page_start;

        // The VMA is page-aligned but the file data starts at an offset
        // within the first page.  The page fault handler uses `offset` as the
        // file position for the VMA's start address, so we need to subtract
        // the page-alignment adjustment from the file offset.  The file_size
        // stays as p_filesz — we don't extend it beyond the original segment.
        // Bytes before p_vaddr within the first page are zero-filled by demand
        // paging (the page fault handler zeroes the page first, then copies
        // file data at the correct offset within the page).
        let start_adjust = seg_start - page_start;
        let area_type = if phdr.p_filesz > 0 {
            VmAreaType::File {
                file: file.clone(),
                // The file offset must correspond to the VMA start (page_start),
                // which is start_adjust bytes before seg_start.  The ELF file
                // offset for seg_start is p_offset, so for page_start it's
                // p_offset - start_adjust.  This is safe because PT_LOAD segments
                // have p_offset page-aligned (ELF spec requirement).
                offset: (phdr.p_offset as usize).wrapping_sub(start_adjust),
                file_size: (phdr.p_filesz as usize) + start_adjust,
            }
        } else {
            VmAreaType::Anonymous
        };
        vm.add_vm_area_with_prot(
            UserVAddr::new_nonnull(page_start)?,
            page_len,
            area_type,
            elf_flags_to_prot(phdr.p_flags),
            false,
        )?;

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

/// Pre-map pages from the page cache into the new address space during exec.
/// For immutable file-backed VMAs (initramfs), this eliminates demand-paging
/// faults (each a ~500ns KVM VM exit) by mapping cached pages directly.
///
/// Two-pass strategy:
///   Pass 1: For each PDE-aligned 2MB region with enough cached pages, map a
///           huge page (from HUGE_PAGE_CACHE, or assembled from 4KB pages).
///           Reduces ~233 TLB entries to 1 for BusyBox text, and enables KVM
///           to use 2MB EPT entries, eliminating costly 2D page walks.
///   Pass 2: Map remaining 4KB cached pages, skipping huge-page-covered regions.
fn prefault_cached_pages(vm: &mut Vm) {
    // Page cache prefaulting re-enabled
    use crate::mm::page_fault::{PAGE_CACHE, huge_page_cache_lookup, huge_page_cache_insert};
    use crate::mm::vm::VmAreaType;
    use alloc::sync::Arc;
    use kevlar_platform::arch::{PAGE_SIZE, HUGE_PAGE_SIZE};
    use kevlar_utils::alignment::align_down;

    let cache_guard = PAGE_CACHE.lock_no_irq();
    let cache_map = match cache_guard.as_ref() {
        Some(map) if !map.is_empty() => map,
        _ => return,
    };

    // Collect VMA info for immutable file-backed VMAs.
    #[allow(dead_code)]
    struct VmaInfo {
        file_ptr: usize,
        file: Arc<dyn kevlar_vfs::inode::FileLike>,
        offset: usize,
        file_size: usize,
        vma_start: usize,
        vma_end: usize,
    }
    let mut vma_infos: Vec<VmaInfo> = Vec::new();
    for vma_idx in 0..vm.vm_areas().len() {
        let vma = &vm.vm_areas()[vma_idx];
        if let VmAreaType::File { file, offset, file_size } = vma.area_type() {
            if file.is_content_immutable() {
                vma_infos.push(VmaInfo {
                    file_ptr: Arc::as_ptr(file) as *const () as usize,
                    file: file.clone(),
                    offset: *offset,
                    file_size: *file_size,
                    vma_start: vma.start().value(),
                    vma_end: vma.end().value(),
                });
            }
        }
    }

    // --- Pass 1: Huge page prefaulting ---
    {
    const PAGES_PER_HUGE: usize = HUGE_PAGE_SIZE / PAGE_SIZE; // 512
    const ASSEMBLE_THRESHOLD: usize = 128;
    {
        // Collect unique PDE-aligned candidates from all VMAs.
        struct HugeCandidate {
            file_ptr: usize,
            huge_vaddr: usize,
        }
        let mut candidates: Vec<HugeCandidate> = Vec::new();

        for info in &vma_infos {
            let vma_page_start = kevlar_utils::alignment::align_up(info.vma_start, PAGE_SIZE);
            let pde_start = align_down(vma_page_start, HUGE_PAGE_SIZE);
            let pde_end = kevlar_utils::alignment::align_up(info.vma_end, HUGE_PAGE_SIZE);

            let mut huge_addr = pde_start;
            while huge_addr < pde_end {
                if !candidates.iter().any(|c| c.huge_vaddr == huge_addr) {
                    candidates.push(HugeCandidate {
                        file_ptr: info.file_ptr,
                        huge_vaddr: huge_addr,
                    });
                }
                huge_addr += HUGE_PAGE_SIZE;
            }
        }

        for cand in &candidates {
            let huge_uaddr = match kevlar_platform::address::UserVAddr::new_nonnull(cand.huge_vaddr) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !vm.page_table().is_pde_empty(huge_uaddr) {
                continue;
            }

            // Safety check: only map a 2MB huge page if the ENTIRE range is
            // covered by immutable file VMAs. If the huge page extends beyond
            // the VMA into address space that will be used by mmap (e.g., for
            // dlopen of ext4 files), the stale initramfs data in the huge page
            // would corrupt the demand-faulted ext4 data.
            let huge_end = cand.huge_vaddr + HUGE_PAGE_SIZE;
            let range_fully_covered = {
                let mut covered = true;
                let mut check_addr = cand.huge_vaddr;
                while check_addr < huge_end {
                    let found = vma_infos.iter().any(|info| {
                        info.vma_start <= check_addr && check_addr < info.vma_end
                    });
                    if !found {
                        covered = false;
                        break;
                    }
                    check_addr += PAGE_SIZE;
                    // Fast skip: jump to the end of the covering VMA
                    for info in &vma_infos {
                        if info.vma_start <= check_addr && check_addr < info.vma_end {
                            check_addr = info.vma_end;
                            break;
                        }
                    }
                }
                covered
            };
            if !range_fully_covered {
                continue;
            }

            // Key the huge page cache by (file_ptr, huge_vaddr) to avoid
            // file-offset-to-vaddr mapping issues across segments.
            let huge_index = cand.huge_vaddr / HUGE_PAGE_SIZE;

            if let Some((huge_paddr, _bitmap)) = huge_page_cache_lookup(cand.file_ptr, huge_index) {
                // Map as a single 2MB huge PDE (RX). One TLB entry instead of ~284.
                // Unpopulated sub-pages are zero-filled (correct for BSS/gap).
                // Write faults (e.g. .data) trigger split_huge_page → CoW path.
                kevlar_platform::page_refcount::page_ref_inc_huge(huge_paddr);
                vm.page_table_mut().map_huge_user_page(huge_uaddr, huge_paddr, 5);
                continue;
            }

            // Count cached 4KB pages in this 2MB virtual region.
            let mut cached_count = 0;
            for i in 0..PAGES_PER_HUGE {
                let sub_vaddr = cand.huge_vaddr + i * PAGE_SIZE;
                for info in &vma_infos {
                    if info.file_ptr != cand.file_ptr {
                        continue;
                    }
                    if sub_vaddr >= info.vma_start && sub_vaddr < info.vma_end {
                        let ov = sub_vaddr - info.vma_start;
                        if ov < info.file_size {
                            let pi = (info.offset + ov) / PAGE_SIZE;
                            if cache_map.contains_key(&(info.file_ptr, pi)) {
                                cached_count += 1;
                            }
                        }
                        break;
                    }
                }
            }
            if cached_count < ASSEMBLE_THRESHOLD {
                continue;
            }

            // Assemble a huge page by populating each 4KB sub-page.
            // For each sub-page at vaddr V = huge_base + i*4K, find the VMA
            // covering V and read file data at the correct offset. Use the
            // 4KB page cache when available (fast path) for cached pages.
            //
            // NOTE: file-to-vaddr mapping differs per ELF segment, so we
            // must look up the VMA per sub-page, not use a linear offset.
            let huge_paddr = match kevlar_platform::page_allocator::alloc_huge_page() {
                Ok(p) => p,
                Err(_) => continue,
            };

            kevlar_platform::page_ops::zero_huge_page(huge_paddr);

            // Track which sub-pages have content via a u64x8 bitmap (512 bits).
            // Using a bitmap instead of [bool; 512] to avoid kernel stack overflow
            // (kernel stack is only 16KB on x86_64).
            let mut populated = [0u64; 8];
            for i in 0..PAGES_PER_HUGE {
                let sub_vaddr = cand.huge_vaddr + i * PAGE_SIZE;
                let sub_end = sub_vaddr + PAGE_SIZE;
                let dst_paddr = kevlar_platform::address::PAddr::new(
                    huge_paddr.value() + i * PAGE_SIZE
                );

                for info in &vma_infos {
                    if info.file_ptr != cand.file_ptr {
                        continue;
                    }
                    if info.vma_end <= sub_vaddr || info.vma_start >= sub_end {
                        continue;
                    }
                    let (offset_in_page, file_offset, copy_len);
                    if sub_vaddr < info.vma_start {
                        // VMA starts mid-page — this page straddles an anonymous
                        // gap VMA and a file VMA. Populate the file portion;
                        // the gap portion is already zero from zero_huge_page.
                        offset_in_page = info.vma_start - sub_vaddr;
                        file_offset = info.offset;
                        copy_len = core::cmp::min(info.file_size, PAGE_SIZE - offset_in_page);
                    } else {
                        let offset_in_vma = sub_vaddr - info.vma_start;
                        offset_in_page = 0;
                        if offset_in_vma >= info.file_size {
                            continue;
                        }
                        file_offset = info.offset + offset_in_vma;
                        copy_len = core::cmp::min(info.file_size - offset_in_vma, PAGE_SIZE);
                    }
                    if copy_len == 0 {
                        continue;
                    }
                    let page_index = file_offset / PAGE_SIZE;

                    // Only use the page cache for full, page-aligned sub-pages.
                    // Boundary/partial pages can't safely use the cache because:
                    // 1. The page_index may collide with a different VMA's partial
                    //    page (e.g. .rodata's last page only has 0x1f0 bytes, rest
                    //    zeros; .data's boundary page at the same index needs
                    //    different content at the overlapping offset).
                    // 2. A full-page copy would overwrite the zero-filled gap.
                    let use_cache = offset_in_page == 0 && copy_len == PAGE_SIZE;
                    if use_cache {
                        if let Some(&src_paddr) = cache_map.get(&(info.file_ptr, page_index)) {
                            #[cfg(not(feature = "profile-fortress"))]
                            {
                                let src = kevlar_platform::page_ops::page_as_slice(src_paddr);
                                let dst = kevlar_platform::page_ops::page_as_slice_mut(dst_paddr);
                                dst.copy_from_slice(src);
                            }
                            #[cfg(feature = "profile-fortress")]
                            {
                                let mut tmp = [0u8; PAGE_SIZE];
                                let src_frame = kevlar_platform::page_ops::PageFrame::new(src_paddr);
                                src_frame.read(0, &mut tmp);
                                let mut dst_frame = kevlar_platform::page_ops::PageFrame::new(dst_paddr);
                                dst_frame.write(0, &tmp);
                            }
                            populated[i / 64] |= 1u64 << (i % 64);
                            break;
                        }
                    }
                    // Cache miss or partial page: read from file.
                    {
                        #[cfg(not(feature = "profile-fortress"))]
                        {
                            let dst = kevlar_platform::page_ops::page_as_slice_mut(dst_paddr);
                            let _ = info.file.read(
                                file_offset,
                                (&mut dst[offset_in_page..(offset_in_page + copy_len)]).into(),
                                &crate::fs::opened_file::OpenOptions::readwrite(),
                            );
                        }
                        #[cfg(feature = "profile-fortress")]
                        {
                            let mut tmp = [0u8; PAGE_SIZE];
                            let _ = info.file.read(
                                file_offset,
                                (&mut tmp[..copy_len]).into(),
                                &crate::fs::opened_file::OpenOptions::readwrite(),
                            );
                            let mut dst_frame = kevlar_platform::page_ops::PageFrame::new(dst_paddr);
                            dst_frame.write(offset_in_page, &tmp[..copy_len]);
                        }
                        populated[i / 64] |= 1u64 << (i % 64);
                    }

                    break;
                }
            }

            let bitmap = populated;

            // Init per-sub-page refcounts: 1 for mapping + 1 for cache.
            kevlar_platform::page_refcount::page_ref_init_huge(huge_paddr);
            kevlar_platform::page_refcount::page_ref_inc_huge(huge_paddr);
            huge_page_cache_insert(cand.file_ptr, huge_index, huge_paddr, bitmap);

            // Map as a single 2MB huge PDE (RX). One TLB entry covers
            // all text/rodata sub-pages. Unpopulated sub-pages are zero-filled
            // (correct for BSS/gap). Write faults trigger split → CoW.
            vm.page_table_mut().map_huge_user_page(huge_uaddr, huge_paddr, 5);
        }
    }

    }

    // --- Pass 2: 4KB page prefaulting ---
    // Skip pages within huge PDE regions (already fully mapped in pass 1).
    for info in &vma_infos {
        let mut addr = if info.vma_start % PAGE_SIZE == 0 {
            info.vma_start
        } else {
            kevlar_utils::alignment::align_up(info.vma_start, PAGE_SIZE)
        };

        while addr + PAGE_SIZE <= info.vma_end && (addr - info.vma_start) < info.file_size {
            // Skip entire 2MB region if a huge PDE covers it.
            let huge_base = align_down(addr, HUGE_PAGE_SIZE);
            if let Ok(huge_uaddr) = kevlar_platform::address::UserVAddr::new_nonnull(huge_base) {
                if vm.page_table().is_huge_mapped(huge_uaddr).is_some() {
                    addr = huge_base + HUGE_PAGE_SIZE;
                    continue;
                }
            }

            let uaddr = kevlar_platform::address::UserVAddr::new_nonnull(addr).unwrap();
            let offset_in_vma = addr - info.vma_start;
            let offset_in_file = info.offset + offset_in_vma;
            let page_index = offset_in_file / PAGE_SIZE;

            if let Some(&cached_paddr) = cache_map.get(&(info.file_ptr, page_index)) {
                if vm.page_table_mut().try_map_user_page_with_prot(uaddr, cached_paddr, 5) {
                    kevlar_platform::page_refcount::page_ref_inc(cached_paddr);
                }
            }

            addr += PAGE_SIZE;
        }
    }

}

/// Pre-map zero pages for small anonymous VMAs (gap padding) and BSS pages
/// (file-backed VMAs past file_size) during exec. These are typically 1-8
/// pages that would otherwise cause demand faults (~1.5µs each under KVM).
/// Skip large anonymous VMAs (stack, heap) to avoid wasting memory.
fn prefault_small_anonymous(vm: &mut Vm) {
    use kevlar_platform::arch::PAGE_SIZE;
    use kevlar_utils::alignment::align_up;

    const MAX_PREFAULT_PAGES: usize = 8;

    for vma_idx in 0..vm.vm_areas().len() {
        let vma = &vm.vm_areas()[vma_idx];
        let prot_flags = vma.prot().bits();
        if prot_flags == 0 {
            continue; // PROT_NONE
        }
        let vma_start = vma.start().value();
        let vma_end = vma.end().value();

        let prefault_start = match vma.area_type() {
            VmAreaType::Anonymous | VmAreaType::DeviceMemory { .. } => {
                // Small anonymous VMAs (gaps, padding).
                let num_pages = (vma_end - vma_start) / PAGE_SIZE;
                if num_pages == 0 || num_pages > MAX_PREFAULT_PAGES {
                    continue;
                }
                align_up(vma_start, PAGE_SIZE)
            }
            VmAreaType::File { file_size, .. } => {
                // BSS tail: zeroed pages past file_size in file-backed VMAs.
                let bss_start = align_up(vma_start + file_size, PAGE_SIZE);
                if bss_start >= vma_end {
                    continue;
                }
                let bss_pages = (vma_end - bss_start) / PAGE_SIZE;
                if bss_pages == 0 || bss_pages > MAX_PREFAULT_PAGES {
                    continue;
                }
                bss_start
            }
        };

        let num_pages = (vma_end - prefault_start) / PAGE_SIZE;
        if num_pages == 0 {
            continue;
        }

        // Batch-allocate pages to amortize allocator lock overhead.
        let mut pages = [kevlar_platform::address::PAddr::new(0); 64];
        let allocated = alloc_page_batch(&mut pages, num_pages);

        // Zero and init refcounts for the batch.
        for i in 0..allocated {
            kevlar_platform::page_ops::zero_page(pages[i]);
            kevlar_platform::page_refcount::page_ref_init(pages[i]);
        }

        // Batch-map: one traversal + one DSB/ISB for up to 32 contiguous
        // pages.  MAX_PREFAULT_PAGES = 8 so the 32-slot cap is plenty.
        let start_uaddr = match UserVAddr::new_nonnull(prefault_start) {
            Ok(v) => v,
            Err(_) => {
                for j in 0..allocated {
                    if kevlar_platform::page_refcount::page_ref_dec(pages[j]) {
                        kevlar_platform::page_allocator::free_pages(pages[j], 1);
                    }
                }
                continue;
            }
        };
        let batch_n = core::cmp::min(allocated, 32);
        let mapped = vm.page_table_mut().batch_try_map_user_pages_with_prot(
            start_uaddr, &pages[..batch_n], batch_n, prot_flags,
        );
        for i in 0..batch_n {
            if mapped & (1u64 << i) == 0 {
                if kevlar_platform::page_refcount::page_ref_dec(pages[i]) {
                    kevlar_platform::page_allocator::free_pages(pages[i], 1);
                }
            }
        }
        // Free anything past the batch cap (shouldn't happen at MAX=8).
        for i in batch_n..allocated {
            if kevlar_platform::page_refcount::page_ref_dec(pages[i]) {
                kevlar_platform::page_allocator::free_pages(pages[i], 1);
            }
        }
    }
}

fn do_elf_binfmt(
    executable: &Arc<dyn FileLike>,
    executable_path: &Arc<PathComponent>,
    argv: &[&[u8]],
    envp: &[&[u8]],
    file_header_pages: kevlar_api::address::PAddr,
    buf: &[u8],
    root_fs: &Arc<SpinLock<RootFs>>,
) -> Result<UserspaceEntry> {
    let file_header_top = USER_STACK_TOP;
    let elf = {
        let _g = crate::debug::tracer::span_guard(crate::debug::tracer::span::EXEC_ELF_PARSE);
        Elf::parse(buf)?
    };

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
    {
        let _g = crate::debug::tracer::span_guard(crate::debug::tracer::span::EXEC_RANDOM);
        read_secure_random(((&mut random_bytes) as &mut [u8]).into())?;
    }

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
    // AT_HWCAP: publish a conservative x86_64 baseline so musl/glibc don't
    // conclude the CPU lacks basic features. FPU + SSE + SSE2 are always
    // present on x86_64. Bits from arch/x86/include/asm/cpufeatures.h.
    //   bit 0  = FPU, bit 23 = MMX, bit 24 = FXSR, bit 25 = SSE, bit 26 = SSE2.
    #[cfg(target_arch = "x86_64")]
    auxv.push(Auxv::Hwcap(0x0078_0001));
    #[cfg(not(target_arch = "x86_64"))]
    auxv.push(Auxv::Hwcap(0));
    auxv.push(Auxv::Clktck(100)); // TICK_HZ — used by glibc's times()/clock()
    auxv.push(Auxv::Uid(0));
    auxv.push(Auxv::Euid(0));
    auxv.push(Auxv::Gid(0));
    auxv.push(Auxv::Egid(0));
    auxv.push(Auxv::Secure(0));
    auxv.push(Auxv::Random(random_bytes));
    auxv.push(Auxv::Hwcap2(0));
    auxv.push(Auxv::MinSigStkSz(2048));
    // AT_PLATFORM: architecture string pointer. Linux always sets this.
    #[cfg(target_arch = "x86_64")]
    auxv.push(Auxv::Platform(b"x86_64"));
    #[cfg(target_arch = "aarch64")]
    auxv.push(Auxv::Platform(b"aarch64"));
    // AT_EXECFN: executable path. Used by musl's __progname_full fallback.
    auxv.push(Auxv::ExecFn(
        executable_path.resolve_absolute_path().as_str().as_bytes().to_vec(),
    ));

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

        prefault_writable_segments(&mut vm, elf.program_headers(), main_base_offset, executable)?;

        // Allocate address range for interpreter and load its segments.
        let interp_base_uaddr = vm.alloc_vaddr_range(interp_span)?;
        let interp_base_offset = interp_base_uaddr.value() - interp_lo;
        trace!("interpreter: base={:#x}, interp_lo={:#x}, interp_hi={:#x}, offset={:#x}",
               interp_base_uaddr.value(), interp_lo, interp_hi, interp_base_offset);

        load_elf_segments(&mut vm, &interp_phdrs, interp_base_offset, &interp_file)?;
        prefault_writable_segments(&mut vm, &interp_phdrs, interp_base_offset, &interp_file)?;

        // Entry point is the interpreter's entry, relocated.
        ip = UserVAddr::new_nonnull(interp_entry_offset + interp_base_offset)?;

        // Update AT_PHDR to point into the loaded main executable image.
        // The dynamic linker computes load bias as (AT_PHDR - phdr[0].p_vaddr).
        // For PIE: main_lo=0, main_base_offset=base → phdr at base + e_phoff.
        // For ET_EXEC: main_base_offset=0, main_lo=0x400000 → phdr at 0x400000 + e_phoff.
        {
            let phdr_addr = main_lo + main_base_offset + (elf.header().e_phoff as usize);
            trace!("AT_PHDR: {:#x} (is_pie={}, main_lo={:#x})", phdr_addr, is_pie, main_lo);
            auxv[0] = Auxv::Phdr(UserVAddr::new_nonnull(phdr_addr)?);
        }

        // Add AT_ENTRY (main exe relocated entry) and AT_BASE (interpreter base).
        auxv.push(Auxv::Entry(main_entry));
        auxv.push(Auxv::Base(interp_base_uaddr.value()));

        // Update heap bottom to be after all loaded images and reserve space
        // so brk doesn't overlap with library mmap allocations.
        let final_top = core::cmp::max(
            main_base_offset + main_hi,
            interp_base_offset + interp_hi,
        );
        let new_heap_bottom = align_up(final_top, PAGE_SIZE);
        vm.set_heap_bottom(UserVAddr::new_nonnull(new_heap_bottom)?);
        // Reserve 256MB for the heap so alloc_vaddr_range skips past it.
        // Without this, mmap returns addresses that overlap with brk,
        // causing the dynamic linker's library pages to be overwritten
        // by malloc's heap allocations (via CoW on read-only pages).
        {
            let heap_reserve_end = align_up(new_heap_bottom + 256 * 1024 * 1024, PAGE_SIZE);
            if heap_reserve_end > vm.valloc_next().value() {
                vm.set_valloc_next(UserVAddr::new(heap_reserve_end).unwrap());
            }
        }

        let phdr_val = match &auxv[0] {
            Auxv::Phdr(v) => v.value(),
            _ => 0,
        };
        trace!("dynamic link: ip={:#x} main_entry={:#x} AT_BASE={:#x} AT_PHDR={:#x} heap={:#x} main_base_offset={:#x}",
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

        vm = {
            let _g = crate::debug::tracer::span_guard(crate::debug::tracer::span::EXEC_VM_NEW);
            Vm::new(
                UserVAddr::new(user_stack_bottom).unwrap(),
                UserVAddr::new(user_heap_bottom).unwrap(),
            )?
        };

        // Map file header pages.
        for i in 0..(buf.len() / PAGE_SIZE) {
            vm.page_table_mut().map_user_page(
                file_header_top_val.sub(((buf.len() / PAGE_SIZE) - i) * PAGE_SIZE),
                file_header_pages.add(i * PAGE_SIZE),
            );
        }

        // Register main executable's PT_LOAD segments.
        // Use VMA template cache when available to skip ELF iteration + gap filling.
        {
            let _g = crate::debug::tracer::span_guard(crate::debug::tracer::span::EXEC_LOAD_SEGMENTS);
            let file_ptr = Arc::as_ptr(executable) as *const () as usize;
            if let Some(template) = vma_template_lookup(file_ptr) {
                apply_vma_template(&mut vm, &template, executable)?;
            } else {
                let vma_count_before = vm.vm_areas().len();
                load_elf_segments(&mut vm, elf.program_headers(), 0, executable)?;
                save_vma_template(file_ptr, &vm, vma_count_before);
            }
        }

        ip = elf.entry()?;
    }

    // Pre-map pages from the page cache into the new address space.
    // This eliminates demand-paging faults for cached initramfs pages,
    // each of which would be a ~500ns KVM VM exit.
    {
        let _g = crate::debug::tracer::span_guard(crate::debug::tracer::span::EXEC_PREFAULT);
        let file_ptr_for_template = Arc::as_ptr(executable) as *const () as usize;
        let use_template = PREFAULT_TEMPLATE_ENABLED.load(Ordering::Relaxed);
        let current_cache_gen = crate::mm::page_fault::PAGE_CACHE_GEN.load(Ordering::Relaxed);
        let template_gen = if use_template {
            prefault_template_lookup(file_ptr_for_template)
        } else {
            None
        };
        if let Some(tpl_gen) = template_gen {
            if tpl_gen == current_cache_gen {
                // Template is fresh — use it directly (skip HashMap lookups).
                let _g2 = crate::debug::tracer::span_guard(crate::debug::tracer::span::EXEC_TEMPLATE);
                apply_prefault_template(&mut vm, file_ptr_for_template);
            } else {
                // Template is stale (cache has new pages) — full prefault + rebuild.
                prefault_cached_pages(&mut vm);
                build_and_save_prefault_template(&vm, file_ptr_for_template);
            }
        } else {
            prefault_cached_pages(&mut vm);
            if use_template {
                build_and_save_prefault_template(&vm, file_ptr_for_template);
            }
        }
        prefault_small_anonymous(&mut vm);
    }

    // Advance valloc_next past all existing VMAs (including gap-fill and
    // page-aligned extents). This ensures that mmap calls from the dynamic
    // linker (during process startup) don't get addresses that overlap with
    // prefaulted interpreter/binary pages.
    {
        let mut max_end = vm.valloc_next().value();
        for vma in vm.vm_areas() {
            let vma_page_end = align_up(vma.end().value(), PAGE_SIZE);
            if vma_page_end > max_end {
                max_end = vma_page_end;
            }
        }
        if max_end > vm.valloc_next().value() {
            vm.set_valloc_next(UserVAddr::new(max_end).unwrap_or(vm.valloc_next()));
        }
    }

    // Map vDSO page (read + execute, no write) into the new address space.
    #[cfg(target_arch = "x86_64")]
    if let Some(vdso_paddr) = kevlar_platform::arch::vdso::page_paddr() {
        let vdso_uaddr = UserVAddr::new(kevlar_platform::arch::vdso::VDSO_VADDR).unwrap();
        vm.page_table_mut().map_user_page_with_prot(vdso_uaddr, vdso_paddr, 5); // PROT_READ|PROT_EXEC
    }

    // Build init stack.
    let user_sp = {
        let _g = crate::debug::tracer::span_guard(crate::debug::tracer::span::EXEC_STACK);
        let init_stack_len = align_up(estimate_user_init_stack_size(argv, envp, &auxv), PAGE_SIZE);
        let init_stack_pages = alloc_pages(init_stack_len / PAGE_SIZE, AllocPageFlags::KERNEL)?;
        let sp = init_user_stack(
            init_stack_top,
            init_stack_pages.as_vaddr().add(init_stack_len),
            init_stack_pages.as_vaddr(),
            argv,
            envp,
            &auxv,
        )?;
        // Batch-map the init stack pages (4KB each).  The init frame is
        // typically 1-4 pages for a small binary, up to ~32 for a shell
        // with a big env — fits in the batch primitive's 32-page cap.
        let n_pages = init_stack_len / PAGE_SIZE;
        let stack_base_uaddr = init_stack_top.sub(n_pages * PAGE_SIZE);
        let mut stack_paddrs = [kevlar_platform::address::PAddr::new(0); 32];
        let batch_n = core::cmp::min(n_pages, 32);
        for i in 0..batch_n {
            stack_paddrs[i] = init_stack_pages.add(i * PAGE_SIZE);
            kevlar_platform::page_refcount::page_ref_init(stack_paddrs[i]);
        }
        let _ = vm.page_table_mut().batch_try_map_user_pages_with_prot(
            stack_base_uaddr, &stack_paddrs[..batch_n], batch_n,
            3, // PROT_READ | PROT_WRITE
        );
        // Fallback for any pages past the batch cap (very unusual).
        for i in batch_n..n_pages {
            vm.page_table_mut().map_user_page(
                init_stack_top.sub((n_pages - i) * PAGE_SIZE),
                init_stack_pages.add(i * PAGE_SIZE),
            );
        }

        sp
    };
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
    let file_header_pages;
    let executable;
    let buf;
    {
        let _g = debug::tracer::span_guard(debug::tracer::span::EXEC_HDR_READ);
        file_header_pages = alloc_pages(file_header_len / PAGE_SIZE, AllocPageFlags::KERNEL)?;
        #[allow(unsafe_code)]
        let buf_mut =
            unsafe { core::slice::from_raw_parts_mut(file_header_pages.as_mut_ptr(), file_header_len) };
        executable = executable_path.inode.as_file()?;
        executable.read(0, buf_mut.into(), &OpenOptions::readwrite())?;
        buf = buf_mut as &[u8];
    }

    if handle_shebang && buf.starts_with(b"#!") && buf.contains(&b'\n') {
        return do_script_binfmt(&executable_path, argv, envp, root_fs, buf);
    }

    do_elf_binfmt(executable, &executable_path, argv, envp,
                  file_header_pages, buf, root_fs)
}

/// PIDs we've already panicked on, to suppress repeated reports
/// (TASK CORRUPT panics).
static LAST_BAD_PIDS: SpinLock<arrayvec::ArrayVec<(i32, u64), 32>> =
    SpinLock::new(arrayvec::ArrayVec::new_const());

/// Scan every suspended task's saved kernel-stack context for the
/// "RIP=0/2 + RBP=0" corruption signature that masks the XFCE crash.
/// To filter out the BlockedSignalable-but-still-racing window between
/// set_state and switch(), we require the bad context to PERSIST for
/// at least 2 consecutive scans on the same (pid, rip) tuple.
pub fn scan_suspended_task_corruption() {
    let pids: alloc::vec::Vec<(PId, alloc::sync::Arc<Process>)> = {
        let table = PROCESSES.lock();
        table.iter().map(|(pid, p)| (*pid, p.clone())).collect()
    };
    let mut new_bad: arrayvec::ArrayVec<(i32, u64), 32> = arrayvec::ArrayVec::new();
    for (pid, proc) in pids {
        // Only check non-running tasks. A task that's currently executing
        // on a CPU has its real RSP/RIP in registers, not in the saved
        // ArchTask context.
        if !matches!(proc.state(), ProcessState::BlockedSignalable | ProcessState::Stopped(_)) {
            continue;
        }
        if let Some((rsp, rip, rbp)) = proc.arch.saved_context_summary() {
            // Saved RIP must be a kernel-text pointer (high half).
            // RBP can be anything — even 0 at thread-entry — so we
            // only check RIP.
            if rip < 0xffff_8000_0000_0000 || rip > 0xffff_8fff_ffff_ffff {
                let owner = proc.arch.rsp_in_owned_stack(rsp);
                // Skip false-positives: if saved_rsp doesn't point into
                // any stack this task owns, the value is dangling
                // (stale from a previous incarnation) and not a real
                // corruption signal.
                if owner.is_none() {
                    continue;
                }
                // Was this same (pid, rsp) reported as bad on the
                // previous scan tick? If yes, the corruption has
                // persisted across at least one full scan interval —
                // confidently real. If not, file it for next scan.
                let prior_bad = LAST_BAD_PIDS.lock_no_irq()
                    .iter().any(|&(p, r)| p == pid.as_i32() && r == rsp);
                if !prior_bad {
                    if !new_bad.is_full() {
                        new_bad.push((pid.as_i32(), rsp));
                    }
                    continue;
                }
                let paddr = proc.arch.kernel_stack_paddr()
                    .map(|p| p.value()).unwrap_or(0);
                log::warn!(
                    "TASK CORRUPT (PERSISTENT): pid={} state={:?} \
                     saved_rsp={:#x} in_stack={:?} saved_rip={:#x} \
                     saved_rbp={:#x} kstack_paddr={:#x}",
                    pid.as_i32(), proc.state(), rsp, owner, rip, rbp, paddr,
                );
                // Log without panicking so the test can keep running and
                // we can correlate detector hits with kernel page faults.
            }
        }
    }
    // Update the "previously seen bad" list so a corruption persisting
    // across two consecutive scans triggers the panic.
    {
        let mut last = LAST_BAD_PIDS.lock_no_irq();
        last.clear();
        for entry in new_bad.iter() {
            if !last.is_full() {
                last.push(*entry);
            }
        }
    }
}

pub fn gc_exited_processes() {
    // Free resources (kernel stacks, vDSO pages) of exited-and-reaped
    // processes. Swap the list out under the lock; drop the Arcs AFTER
    // the lock is released AND with interrupts enabled. The Arc drops
    // may trigger Vm::Drop → teardown_user_pages → TLB shootdown IPI,
    // which requires IF=1 (waits for remote CPU ACK). If IF=0 (e.g.
    // called from the IRQ bottom half), flush_tlb_for_teardown falls
    // back to bump-PCID-gen-only, which doesn't invalidate entries on
    // CPUs currently scheduling a different process — so freed PT
    // pages can be corrupted by stale writes.
    let to_drop: alloc::vec::Vec<Arc<Process>> = {
        let mut exited = EXITED_PROCESSES.lock();
        if exited.is_empty() {
            return;
        }
        core::mem::take(&mut *exited)
    }; // lock released (IF restored to caller's state)

    // Ensure IF=1 for the Arc drops. If our caller (e.g. the IRQ bottom
    // half) had IF=0, temporarily enable it here. The drops only touch
    // the kernel heap and the TLB shootdown machinery; neither conflicts
    // with nested IRQs on this CPU.
    let was_if_off = !kevlar_platform::arch::interrupts_enabled();
    if was_if_off {
        kevlar_platform::arch::enable_interrupts();
    }
    drop(to_drop);
    // Also drain any teardowns that other IF=0 drop sites deferred here.
    crate::mm::vm::process_deferred_vm_teardowns();
    if was_if_off {
        #[allow(unsafe_code)]
        unsafe {
            #[cfg(target_arch = "x86_64")]
            core::arch::asm!("cli", options(nomem, nostack));
            #[cfg(target_arch = "aarch64")]
            core::arch::asm!("msr daifset, #2", options(nomem, nostack));
        }
    }
}
