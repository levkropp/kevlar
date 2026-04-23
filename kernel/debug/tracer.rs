// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Lightweight span tracer for kernel phase profiling.
//!
//! Records aggregated timing for named code spans (enter/exit pairs).
//! Follows the same pattern as `profiler.rs`: static array of atomics,
//! gated by a single `AtomicBool`, zero overhead when disabled.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_SPANS: usize = 64;

pub mod span {
    // exec path phases
    pub const EXEC_TOTAL: u16 = 0;
    pub const EXEC_SETUP_USERSPACE: u16 = 1;
    pub const EXEC_ELF_BINFMT: u16 = 2;
    pub const EXEC_VM_NEW: u16 = 3;
    pub const EXEC_LOAD_SEGMENTS: u16 = 4;
    pub const EXEC_PREFAULT: u16 = 5;
    pub const EXEC_STACK: u16 = 6;
    pub const EXEC_RANDOM: u16 = 7;
    pub const EXEC_HDR_READ: u16 = 8;
    pub const EXEC_DE_THREAD: u16 = 9;
    // fork path phases
    pub const FORK_TOTAL: u16 = 10;
    pub const FORK_PAGE_TABLE: u16 = 11;
    pub const FORK_PROCESS_SETUP: u16 = 12;
    // page fault
    pub const PAGE_FAULT_TOTAL: u16 = 13;
    // path resolution
    pub const PATH_LOOKUP: u16 = 14;
    // exec sub-phases
    pub const EXEC_ELF_PARSE: u16 = 15;
    pub const EXEC_SIGNAL_RESET: u16 = 16;
    pub const EXEC_CLOSE_CLOEXEC: u16 = 17;
    // process exit/wait
    pub const EXIT_TOTAL: u16 = 18;
    pub const WAIT_TOTAL: u16 = 19;
    // demand fault experiments
    pub const FORK_GHOST: u16 = 20;
    pub const EXEC_TEMPLATE: u16 = 21;
}

struct SpanStats {
    total_cycles: AtomicU64,
    count: AtomicU64,
    min_cycles: AtomicU64,
    max_cycles: AtomicU64,
}

impl SpanStats {
    const fn new() -> Self {
        Self {
            total_cycles: AtomicU64::new(0),
            count: AtomicU64::new(0),
            min_cycles: AtomicU64::new(u64::MAX),
            max_cycles: AtomicU64::new(0),
        }
    }

    fn record(&self, cycles: u64) {
        self.total_cycles.fetch_add(cycles, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut cur = self.min_cycles.load(Ordering::Relaxed);
        while cycles < cur {
            match self.min_cycles.compare_exchange_weak(cur, cycles, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
        let mut cur = self.max_cycles.load(Ordering::Relaxed);
        while cycles > cur {
            match self.max_cycles.compare_exchange_weak(cur, cycles, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}

macro_rules! stats_array {
    ($n:expr) => {{ const INIT: SpanStats = SpanStats::new(); [INIT; $n] }};
}

static STATS: [SpanStats; MAX_SPANS] = stats_array!(MAX_SPANS);
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable() { ENABLED.store(true, Ordering::Release); }

#[inline(always)]
pub fn is_enabled() -> bool { ENABLED.load(Ordering::Relaxed) }

#[inline(always)]
pub fn span_enter() -> u64 {
    if !is_enabled() { return 0; }
    kevlar_platform::arch::read_clock_counter()
}

#[inline(always)]
pub fn span_exit(id: u16, start_tsc: u64) {
    if start_tsc == 0 || (id as usize) >= MAX_SPANS { return; }
    let end = kevlar_platform::arch::read_clock_counter();
    STATS[id as usize].record(end.saturating_sub(start_tsc));
}

pub struct SpanGuard { id: u16, start: u64 }

impl Drop for SpanGuard {
    #[inline(always)]
    fn drop(&mut self) { span_exit(self.id, self.start); }
}

#[inline(always)]
pub fn span_guard(id: u16) -> SpanGuard {
    SpanGuard { id, start: span_enter() }
}

fn span_name(id: u16) -> &'static str {
    match id {
        span::EXEC_TOTAL => "exec.total",
        span::EXEC_SETUP_USERSPACE => "exec.setup_userspace",
        span::EXEC_ELF_BINFMT => "exec.elf_binfmt",
        span::EXEC_VM_NEW => "exec.vm_new",
        span::EXEC_LOAD_SEGMENTS => "exec.load_segments",
        span::EXEC_PREFAULT => "exec.prefault",
        span::EXEC_STACK => "exec.stack",
        span::EXEC_RANDOM => "exec.random",
        span::EXEC_HDR_READ => "exec.hdr_read",
        span::EXEC_DE_THREAD => "exec.de_thread",
        span::FORK_TOTAL => "fork.total",
        span::FORK_PAGE_TABLE => "fork.page_table",
        span::FORK_PROCESS_SETUP => "fork.process_setup",
        span::PAGE_FAULT_TOTAL => "page_fault.total",
        span::PATH_LOOKUP => "path.lookup",
        span::EXEC_ELF_PARSE => "exec.elf_parse",
        span::EXEC_SIGNAL_RESET => "exec.signal_reset",
        span::EXEC_CLOSE_CLOEXEC => "exec.close_cloexec",
        span::EXIT_TOTAL => "exit.total",
        span::WAIT_TOTAL => "wait.total",
        span::FORK_GHOST => "fork.ghost",
        span::EXEC_TEMPLATE => "exec.template",
        _ => "unknown",
    }
}

pub fn dump_span_profile() {
    let freq = tsc_freq_hz();
    if freq == 0 { return; }
    print!("DBG {{\"type\":\"span_profile\",\"tsc_freq_hz\":{},\"entries\":[", freq);
    let mut first = true;
    for id in 0..MAX_SPANS {
        let count = STATS[id].count.load(Ordering::Relaxed);
        if count == 0 { continue; }
        let total = STATS[id].total_cycles.load(Ordering::Relaxed);
        let min = STATS[id].min_cycles.load(Ordering::Relaxed);
        let max = STATS[id].max_cycles.load(Ordering::Relaxed);
        let avg = total / count;
        let avg_ns = cycles_to_ns(avg, freq);
        let min_ns = cycles_to_ns(min, freq);
        let max_ns = cycles_to_ns(max, freq);
        let total_ns = cycles_to_ns(total, freq);
        if !first { print!(","); }
        first = false;
        print!(
            "{{\"id\":{},\"name\":\"{}\",\"calls\":{},\"total_ns\":{},\"avg_ns\":{},\"min_ns\":{},\"max_ns\":{}}}",
            id, span_name(id as u16), count, total_ns, avg_ns, min_ns, max_ns
        );
    }
    println!("]}}");
}

fn tsc_freq_hz() -> u64 {
    kevlar_platform::arch::read_clock_frequency()
}

fn cycles_to_ns(cycles: u64, freq: u64) -> u64 {
    let secs = cycles / freq;
    let remainder = cycles % freq;
    secs * 1_000_000_000 + remainder * 1_000_000_000 / freq
}
