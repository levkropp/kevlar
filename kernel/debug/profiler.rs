// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-syscall cycle counter profiler.
//!
//! When enabled, records TSC cycles at syscall entry and exit, accumulating
//! per-syscall-number statistics: total cycles, call count, min, and max.
//! Near-zero overhead when disabled (single atomic load).
//!
//! # Usage
//!
//! Enable via kernel command line: `debug=profile` or at runtime.
//! Dump with `dump_syscall_profile()` (called on clean shutdown or via MCP).
//!
//! # Output
//!
//! ```text
//! DBG {"type":"syscall_profile","entries":[{"nr":39,"name":"getpid","calls":10000,"total_ns":1960000,"avg_ns":196,"min_ns":180,"max_ns":350},...]}
//! ```

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Maximum syscall number we track.  Higher numbers are silently ignored.
const MAX_NR: usize = 512;

/// Per-syscall statistics bucket.
struct SyscallStats {
    total_cycles: AtomicU64,
    count: AtomicU64,
    min_cycles: AtomicU64,
    max_cycles: AtomicU64,
}

impl SyscallStats {
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

        // Update min/max with compare-and-swap loops.
        let mut current_min = self.min_cycles.load(Ordering::Relaxed);
        while cycles < current_min {
            match self.min_cycles.compare_exchange_weak(
                current_min, cycles, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => current_min = v,
            }
        }

        let mut current_max = self.max_cycles.load(Ordering::Relaxed);
        while cycles > current_max {
            match self.max_cycles.compare_exchange_weak(
                current_max, cycles, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => current_max = v,
            }
        }
    }
}

// Use a macro to generate the array initialization since SyscallStats
// doesn't implement Copy (it contains atomics).
macro_rules! stats_array {
    ($n:expr) => {{
        // SAFETY: AtomicU64::new(0) is all-zeros, AtomicU64::new(u64::MAX) is
        // all-ones.  We zero-initialize then fix up min_cycles.
        const INIT: SyscallStats = SyscallStats::new();
        [INIT; $n]
    }};
}

static STATS: [SyscallStats; MAX_NR] = stats_array!(MAX_NR);
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable the syscall profiler.
pub fn enable() {
    ENABLED.store(true, Ordering::Release);
}

/// Disable the syscall profiler.
pub fn disable() {
    ENABLED.store(false, Ordering::Release);
}

/// Check if profiling is enabled.
#[inline(always)]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Record the start of a syscall.  Returns the TSC value (or 0 if disabled).
#[inline(always)]
pub fn syscall_enter() -> u64 {
    if !is_enabled() {
        return 0;
    }
    read_tsc()
}

/// Record the end of a syscall.
#[inline(always)]
pub fn syscall_exit(nr: usize, start_tsc: u64) {
    if start_tsc == 0 || nr >= MAX_NR {
        return;
    }
    let end = read_tsc();
    let cycles = end.saturating_sub(start_tsc);
    STATS[nr].record(cycles);
}

/// Dump the syscall profile as a structured debug event.
pub fn dump_syscall_profile(syscall_name_fn: fn(usize) -> &'static str) {
    let freq = tsc_freq_hz();
    if freq == 0 {
        return;
    }

    // Use the kernel's print infrastructure directly — this is a one-shot
    // dump, not a hot-path event.
    println!("DBG {{\"type\":\"syscall_profile\",\"tsc_freq_hz\":{},\"entries\":[", freq);

    let mut first = true;
    for nr in 0..MAX_NR {
        let count = STATS[nr].count.load(Ordering::Relaxed);
        if count == 0 {
            continue;
        }

        let total = STATS[nr].total_cycles.load(Ordering::Relaxed);
        let min = STATS[nr].min_cycles.load(Ordering::Relaxed);
        let max = STATS[nr].max_cycles.load(Ordering::Relaxed);
        let avg = total / count;

        // Convert cycles to nanoseconds: ns = cycles * 1_000_000_000 / freq
        let avg_ns = cycles_to_ns(avg, freq);
        let min_ns = cycles_to_ns(min, freq);
        let max_ns = cycles_to_ns(max, freq);
        let total_ns = cycles_to_ns(total, freq);

        let name = syscall_name_fn(nr);

        if !first {
            print!(",");
        }
        first = false;

        println!(
            "{{\"nr\":{},\"name\":\"{}\",\"calls\":{},\"total_ns\":{},\"avg_ns\":{},\"min_ns\":{},\"max_ns\":{}}}",
            nr, name, count, total_ns, avg_ns, min_ns, max_ns
        );
    }
    println!("]}}");
}

#[inline(always)]
fn read_tsc() -> u64 {
    kevlar_platform::arch::read_clock_counter()
}

fn tsc_freq_hz() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        kevlar_platform::arch::tsc::frequency_hz()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

fn cycles_to_ns(cycles: u64, freq: u64) -> u64 {
    let secs = cycles / freq;
    let remainder = cycles % freq;
    secs * 1_000_000_000 + remainder * 1_000_000_000 / freq
}
