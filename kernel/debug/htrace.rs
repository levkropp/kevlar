// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Hierarchical call tracer — records nested kernel call chains per-CPU.
//!
//! When enabled (`KEVLAR_DEBUG=htrace` or `debug=htrace`), records the last
//! 4096 enter/exit events per CPU into a lock-free ring buffer. Each entry
//! captures a TSC timestamp, a function/span ID, the nesting depth, and
//! 32 bits of context data (pid, errno, fd, etc.).
//!
//! The trace can be dumped to serial as JSONL, showing the full call hierarchy
//! with timing for each span. This is the primary tool for debugging:
//! - Why epoll_wait blocks forever (sleep → wake → callback chain)
//! - Why SIGSEGV delivery fails (page fault → signal → trampoline chain)
//! - Why signal handlers aren't invoked (alarm → send_signal → delivery chain)
//!
//! # Overhead
//! - When disabled: one atomic load (filter check) per instrumentation site
//! - When enabled: ~30ns per enter/exit (rdtsc + atomic store to ring buffer)
//!
//! # Usage
//! ```rust
//! use crate::debug::htrace;
//!
//! fn my_function() {
//!     let _g = htrace::enter_guard(htrace::id::EPOLL_WAIT, pid as u32);
//!     // ... nested calls also traced ...
//! }
//! ```
//!
//! Dump the trace from panic handler or on demand:
//! ```rust
//! htrace::dump();          // dump current CPU
//! htrace::dump_all_cpus(); // dump all CPUs (after halt IPI)
//! ```

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Whether the hierarchical tracer is active. Checked on every enter/exit.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable the tracer. Called from `debug::init()` when `htrace` filter is set.
pub fn enable() {
    ENABLED.store(true, Ordering::Release);
}

/// Check if tracing is enabled (one atomic load — fast path).
#[inline(always)]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

// ── Trace entry ─────────────────────────────────────────────────────

/// A single trace entry: 16 bytes, fits in one cache line pair.
#[derive(Copy, Clone)]
#[repr(C)]
pub struct Entry {
    /// TSC timestamp (rdtsc).
    pub tsc: u64,
    /// Span/function ID (see `id` module).
    pub id: u16,
    /// Nesting depth (0 = top-level syscall).
    pub depth: u8,
    /// Flags: bit 0 = EXIT (0=enter, 1=exit), bits 1-7 reserved.
    pub flags: u8,
    /// Context-specific data (pid, errno, fd number, address low bits, etc.).
    pub data: u32,
}

impl Entry {
    const ZERO: Entry = Entry { tsc: 0, id: 0, depth: 0, flags: 0, data: 0 };
}

const FLAG_EXIT: u8 = 0x01;

// ── Per-CPU ring buffer ─────────────────────────────────────────────

const RING_SIZE: usize = 4096;
const MAX_CPUS: usize = 8;

/// Per-CPU trace state. Aligned to cache line to avoid false sharing.
#[repr(align(64))]
struct CpuTrace {
    ring: [Entry; RING_SIZE],
    /// Write index (wraps around).
    write_idx: AtomicUsize,
    /// Current nesting depth.
    depth: AtomicU32,
}

impl CpuTrace {
    const fn new() -> CpuTrace {
        CpuTrace {
            ring: [Entry::ZERO; RING_SIZE],
            write_idx: AtomicUsize::new(0),
            depth: AtomicU32::new(0),
        }
    }
}

/// All per-CPU trace buffers. Index by `cpu_id()`.
static mut CPU_TRACES: [CpuTrace; MAX_CPUS] = [
    CpuTrace::new(), CpuTrace::new(), CpuTrace::new(), CpuTrace::new(),
    CpuTrace::new(), CpuTrace::new(), CpuTrace::new(), CpuTrace::new(),
];

/// Get the trace buffer for the current CPU.
///
/// # Safety
/// Each CPU only writes to its own buffer (indexed by cpu_id). Reads during
/// dump happen after halt IPI freezes other CPUs, or are best-effort.
#[inline(always)]
#[allow(unsafe_code)]
fn cpu_trace() -> &'static CpuTrace {
    let cpu = kevlar_platform::arch::cpu_id() as usize;
    debug_assert!(cpu < MAX_CPUS);
    unsafe { &CPU_TRACES[cpu % MAX_CPUS] }
}

// ── Record enter/exit ───────────────────────────────────────────────

/// Record a span entry. Returns the current depth (for verification).
#[inline(always)]
pub fn enter(id: u16, data: u32) {
    if !is_enabled() { return; }

    let ct = cpu_trace();
    let depth = ct.depth.fetch_add(1, Ordering::Relaxed) as u8;
    let tsc = kevlar_platform::arch::read_clock_counter();
    let idx = ct.write_idx.fetch_add(1, Ordering::Relaxed) % RING_SIZE;

    // SAFETY: only this CPU writes to this index (single-producer ring).
    #[allow(unsafe_code)]
    unsafe {
        let ptr = &CPU_TRACES[kevlar_platform::arch::cpu_id() as usize % MAX_CPUS]
            .ring as *const _ as *mut [Entry; RING_SIZE];
        (*ptr)[idx] = Entry { tsc, id, depth, flags: 0, data };
    }
}

/// Record a span exit.
#[inline(always)]
pub fn exit(id: u16, data: u32) {
    if !is_enabled() { return; }

    let ct = cpu_trace();
    let depth = ct.depth.fetch_sub(1, Ordering::Relaxed).saturating_sub(1) as u8;
    let tsc = kevlar_platform::arch::read_clock_counter();
    let idx = ct.write_idx.fetch_add(1, Ordering::Relaxed) % RING_SIZE;

    #[allow(unsafe_code)]
    unsafe {
        let ptr = &CPU_TRACES[kevlar_platform::arch::cpu_id() as usize % MAX_CPUS]
            .ring as *const _ as *mut [Entry; RING_SIZE];
        (*ptr)[idx] = Entry { tsc, id, depth, flags: FLAG_EXIT, data };
    }
}

// ── RAII guard ──────────────────────────────────────────────────────

/// RAII guard that calls `exit()` when dropped.
pub struct Guard {
    id: u16,
    data: u32,
    active: bool,
}

impl Drop for Guard {
    #[inline(always)]
    fn drop(&mut self) {
        if self.active {
            exit(self.id, self.data);
        }
    }
}

/// Create an RAII guard that records enter now and exit on drop.
/// Zero-cost when tracing is disabled.
#[inline(always)]
pub fn enter_guard(id: u16, data: u32) -> Guard {
    let active = is_enabled();
    if active {
        enter(id, data);
    }
    Guard { id, data, active }
}

// ── Dump ────────────────────────────────────────────────────────────

/// Dump the trace for the current CPU to serial as JSONL.
/// Each line: `DBG {"type":"htrace","cpu":N,"tsc":T,"id":I,"name":"...","depth":D,"dir":"enter"|"exit","data":X}`
pub fn dump() {
    dump_cpu(kevlar_platform::arch::cpu_id() as usize);
}

/// Dump traces for all CPUs (call after halt IPI).
pub fn dump_all_cpus() {
    let ncpus = kevlar_platform::arch::num_online_cpus() as usize;
    for cpu in 0..ncpus.min(MAX_CPUS) {
        dump_cpu(cpu);
    }
}

fn dump_cpu(cpu: usize) {
    #[allow(unsafe_code)]
    let ct = unsafe { &CPU_TRACES[cpu % MAX_CPUS] };
    let write_idx = ct.write_idx.load(Ordering::Relaxed);

    // Determine how many entries to dump (up to RING_SIZE).
    let count = write_idx.min(RING_SIZE);
    let start = if write_idx >= RING_SIZE { write_idx - RING_SIZE } else { 0 };

    for i in start..write_idx {
        let entry = ct.ring[i % RING_SIZE];
        if entry.tsc == 0 { continue; } // uninitialized slot

        let dir = if entry.flags & FLAG_EXIT != 0 { "exit" } else { "enter" };
        let name = id::name(entry.id);

        // Indent by depth for readability.
        let indent_count = (entry.depth as usize).min(16);
        let indent = &"                "[..indent_count];

        kevlar_platform::println!(
            "DBG {{\"type\":\"htrace\",\"cpu\":{},\"tsc\":{},\"id\":{},\"name\":\"{}\",\"depth\":{},\"dir\":\"{}\",\"data\":{}}}",
            cpu, entry.tsc, entry.id, name, entry.depth, dir, entry.data
        );
        let _ = indent; // used for pretty-print mode if needed
    }

    kevlar_platform::println!(
        "DBG {{\"type\":\"htrace_summary\",\"cpu\":{},\"total_entries\":{},\"ring_size\":{}}}",
        cpu, write_idx, RING_SIZE
    );
}

// ── Span ID constants ───────────────────────────────────────────────

pub mod id {
    //! Predefined span IDs. Add new ones here as needed.
    //!
    //! Convention: group by subsystem in ranges of 16.

    // ── Syscall dispatch (0-15) ──
    pub const SYSCALL: u16 = 0;
    pub const SYSCALL_SIGNAL_CHECK: u16 = 1;

    // ── Page fault (16-31) ──
    pub const PAGE_FAULT: u16 = 16;
    pub const PAGE_FAULT_VMA_LOOKUP: u16 = 17;
    pub const PAGE_FAULT_ALLOC: u16 = 18;
    pub const PAGE_FAULT_MAP: u16 = 19;
    pub const PAGE_FAULT_COW: u16 = 20;
    pub const PAGE_FAULT_DEMAND: u16 = 21;
    pub const PAGE_FAULT_SIGNAL: u16 = 22;

    // ── Signal delivery (32-47) ──
    pub const SIGNAL_SEND: u16 = 32;
    pub const SIGNAL_DELIVER: u16 = 33;
    pub const SIGNAL_SETUP_FRAME: u16 = 34;
    pub const SIGNAL_CHECK_PENDING: u16 = 35;
    pub const SIGNAL_DEFAULT_ACTION: u16 = 36;

    // ── Process lifecycle (48-63) ──
    pub const FORK: u16 = 48;
    pub const EXEC: u16 = 49;
    pub const EXIT: u16 = 50;
    pub const WAIT: u16 = 51;

    // ── Sleep / wake (64-79) ──
    pub const SLEEP_UNTIL: u16 = 64;
    pub const SLEEP_CALLBACK: u16 = 65;
    pub const WAKE_ALL: u16 = 66;
    pub const WAKE_ONE: u16 = 67;
    pub const TIMER_IRQ: u16 = 68;

    // ── epoll / poll / select (80-95) ──
    pub const EPOLL_WAIT: u16 = 80;
    pub const EPOLL_COLLECT: u16 = 81;
    pub const EPOLL_CTL: u16 = 82;
    pub const POLL_FD: u16 = 83;
    pub const SELECT_CHECK: u16 = 84;

    // ── VM operations (96-111) ──
    pub const MMAP: u16 = 96;
    pub const MUNMAP: u16 = 97;
    pub const MPROTECT: u16 = 98;
    pub const MADVISE: u16 = 99;
    pub const BRK: u16 = 100;

    // ── Lock operations (112-127) ──
    pub const LOCK_ACQUIRE: u16 = 112;
    pub const LOCK_RELEASE: u16 = 113;
    pub const LOCK_FD_TABLE: u16 = 114;
    pub const LOCK_VM: u16 = 115;

    // ── File operations (128-143) ──
    pub const PIPE_READ: u16 = 128;
    pub const PIPE_WRITE: u16 = 129;
    pub const FILE_POLL: u16 = 130;

    /// Map span ID to name. Returns "unknown" for unregistered IDs.
    pub fn name(id: u16) -> &'static str {
        match id {
            SYSCALL => "syscall",
            SYSCALL_SIGNAL_CHECK => "syscall.signal_check",
            PAGE_FAULT => "page_fault",
            PAGE_FAULT_VMA_LOOKUP => "page_fault.vma_lookup",
            PAGE_FAULT_ALLOC => "page_fault.alloc",
            PAGE_FAULT_MAP => "page_fault.map",
            PAGE_FAULT_COW => "page_fault.cow",
            PAGE_FAULT_DEMAND => "page_fault.demand",
            PAGE_FAULT_SIGNAL => "page_fault.signal",
            SIGNAL_SEND => "signal.send",
            SIGNAL_DELIVER => "signal.deliver",
            SIGNAL_SETUP_FRAME => "signal.setup_frame",
            SIGNAL_CHECK_PENDING => "signal.check_pending",
            SIGNAL_DEFAULT_ACTION => "signal.default_action",
            FORK => "fork",
            EXEC => "exec",
            EXIT => "exit",
            WAIT => "wait",
            SLEEP_UNTIL => "sleep.until",
            SLEEP_CALLBACK => "sleep.callback",
            WAKE_ALL => "wake.all",
            WAKE_ONE => "wake.one",
            TIMER_IRQ => "timer_irq",
            EPOLL_WAIT => "epoll.wait",
            EPOLL_COLLECT => "epoll.collect",
            EPOLL_CTL => "epoll.ctl",
            POLL_FD => "poll.fd",
            SELECT_CHECK => "select.check",
            MMAP => "mmap",
            MUNMAP => "munmap",
            MPROTECT => "mprotect",
            MADVISE => "madvise",
            BRK => "brk",
            LOCK_ACQUIRE => "lock.acquire",
            LOCK_RELEASE => "lock.release",
            LOCK_FD_TABLE => "lock.fd_table",
            LOCK_VM => "lock.vm",
            PIPE_READ => "pipe.read",
            PIPE_WRITE => "pipe.write",
            FILE_POLL => "file.poll",
            _ => "unknown",
        }
    }
}
