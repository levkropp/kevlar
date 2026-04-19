// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::handler;

use super::gdt::{KERNEL_CS, USER_CS32};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use x86::msr::{self, rdmsr, wrmsr};

// Mask RFLAGS bits on SYSCALL entry (IA32_FMASK).  The CPU clears these
// bits in RFLAGS when SYSCALL executes:
//   IF  (0x0200) — disable interrupts before SWAPGS
//   TF  (0x0100) — prevent single-step #DB flood in kernel mode
//   DF  (0x0400) — direction flag (string ops forward)
// Note: Linux also masks NT and AC, but we keep it minimal for now.
const SYSCALL_RFLAGS_MASK: u64 = 0x0700;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct PtRegs {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[unsafe(no_mangle)]
extern "C" fn x64_handle_syscall(
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
    n: usize,
    frame: *mut PtRegs,
) -> isize {
    let cpu = super::cpu_id() as usize;
    if cpu < SYSCALL_COUNT.len() {
        SYSCALL_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
        LAST_SYSCALL_NR[cpu].store(n as u32, Ordering::Relaxed);
    }
    // Histogram: bracket the real handler with two TSC reads. Accumulate
    // per-(cpu, nr) sum + count + max for dump_histogram() to emit later
    // (typically from the NMI handler when diagnosing a livelock).
    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    let ret = handler().handle_syscall(a1, a2, a3, a4, a5, a6, n, frame);
    let dt = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(t0);
    if cpu < HIST_CPUS && n < HIST_NRS {
        SYSCALL_LAT_TSC_SUM[cpu][n].fetch_add(dt, Ordering::Relaxed);
        SYSCALL_LAT_CNT[cpu][n].fetch_add(1, Ordering::Relaxed);
        let mut cur = SYSCALL_LAT_MAX[cpu][n].load(Ordering::Relaxed);
        while dt > cur {
            match SYSCALL_LAT_MAX[cpu][n].compare_exchange_weak(
                cur, dt, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(seen) => cur = seen,
            }
        }
    }
    ret
}

/// Per-CPU syscall counter, bumped on every syscall entry. Available via
/// `kevlar_platform::arch::syscall_counter_read(cpu)` — useful when
/// diagnosing whether a CPU has stopped making syscalls (e.g. IF=0 lockup).
pub static SYSCALL_COUNT: [AtomicUsize; 8] = [
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
];

/// Per-CPU last-syscall-number, updated on every syscall entry. Combined
/// with `SYSCALL_COUNT`, exposes "CPU N is stuck inside syscall nr=X" to
/// diagnostic dumps without requiring a live debugger.
pub static LAST_SYSCALL_NR: [AtomicU32; 8] = [
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
];

// ── Per-(cpu, syscall_nr) latency histogram ─────────────────────────────
//
// Lock-free accumulation: every `x64_handle_syscall` invocation brackets
// `handler().handle_syscall` with two `_rdtsc` reads and adds the delta
// into the (cpu, nr) bucket.  Storage lives in `.bss` (40 KiB), zero-cost
// when not dumped.  `dump_histogram(cpu)` produces greppable
// `SYSCALL_HIST ...` lines — safe to call from NMI context because it
// uses only `log::warn!` (same as the existing NMI handler) and a
// fixed-size on-stack sort array, no allocation.

pub const HIST_CPUS: usize = 8;
pub const HIST_NRS: usize = 256;

/// Sum of TSC-ticks spent in each (cpu, syscall_nr). A TSC-wrap at 3 GHz
/// would take ~195 years; safe to treat as monotonic for the test run.
pub static SYSCALL_LAT_TSC_SUM: [[AtomicU64; HIST_NRS]; HIST_CPUS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    const R: [AtomicU64; HIST_NRS] = [Z; HIST_NRS];
    [R, R, R, R, R, R, R, R]
};

/// Number of syscalls seen per (cpu, syscall_nr).
pub static SYSCALL_LAT_CNT: [[AtomicU32; HIST_NRS]; HIST_CPUS] = {
    const Z: AtomicU32 = AtomicU32::new(0);
    const R: [AtomicU32; HIST_NRS] = [Z; HIST_NRS];
    [R, R, R, R, R, R, R, R]
};

/// Worst-case single-call TSC delta per (cpu, syscall_nr). Updated via
/// cmpxchg loop so a single thread's pathological outlier dominates the
/// bucket and lets the reader pinpoint which syscall is slow.
pub static SYSCALL_LAT_MAX: [[AtomicU64; HIST_NRS]; HIST_CPUS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    const R: [AtomicU64; HIST_NRS] = [Z; HIST_NRS];
    [R, R, R, R, R, R, R, R]
};

/// Dump the top-N most expensive syscalls (by average TSC) for `cpu`.
/// Safe to call from NMI context: uses only `log::warn!` + a fixed-size
/// stack array for the partial sort.  No allocation, no lock held across
/// the call — the NMI handler's existing `log::warn!` uses the same
/// `spin::Mutex`-backed printer; this function adds no new lock dependency.
pub fn dump_histogram(cpu: usize) {
    if cpu >= HIST_CPUS {
        return;
    }

    // Collect non-empty buckets into a fixed-size top-N by average TSC.
    // Using `N = 16` keeps the partial sort cheap (O(HIST_NRS * N)) and
    // the on-stack state bounded.
    const TOP_N: usize = 16;

    #[derive(Copy, Clone)]
    struct Row {
        nr:      u32,
        cnt:     u32,
        sum_tsc: u64,
        max_tsc: u64,
        avg_tsc: u64,
    }
    const EMPTY: Row = Row { nr: 0, cnt: 0, sum_tsc: 0, max_tsc: 0, avg_tsc: 0 };
    let mut top: [Row; TOP_N] = [EMPTY; TOP_N];
    let mut filled: usize = 0;
    let mut total_samples: u64 = 0;

    for n in 0..HIST_NRS {
        let cnt = SYSCALL_LAT_CNT[cpu][n].load(Ordering::Relaxed);
        if cnt == 0 {
            continue;
        }
        total_samples = total_samples.saturating_add(cnt as u64);
        let sum = SYSCALL_LAT_TSC_SUM[cpu][n].load(Ordering::Relaxed);
        let max = SYSCALL_LAT_MAX[cpu][n].load(Ordering::Relaxed);
        let avg = sum / (cnt as u64);
        let row = Row { nr: n as u32, cnt, sum_tsc: sum, max_tsc: max, avg_tsc: avg };

        // Insert into sorted top-N (descending by avg_tsc).
        if filled < TOP_N {
            top[filled] = row;
            filled += 1;
        } else if avg > top[TOP_N - 1].avg_tsc {
            top[TOP_N - 1] = row;
        } else {
            continue;
        }
        // Bubble up into position.  At most TOP_N swaps per insert.
        let mut i = filled.saturating_sub(1).min(TOP_N - 1);
        while i > 0 && top[i].avg_tsc > top[i - 1].avg_tsc {
            top.swap(i, i - 1);
            i -= 1;
        }
    }

    log::warn!("SYSCALL_HIST cpu={} total_samples={}", cpu, total_samples);
    for i in 0..filled {
        let r = top[i];
        log::warn!(
            "SYSCALL_HIST cpu={} nr={} cnt={} avg_tsc={} max_tsc={} sum_tsc={}",
            cpu, r.nr, r.cnt, r.avg_tsc, r.max_tsc, r.sum_tsc,
        );
    }
}

unsafe extern "C" {
    fn syscall_entry();
}

pub unsafe fn init() {
    wrmsr(
        msr::IA32_STAR,
        ((USER_CS32 as u64) << 48) | ((KERNEL_CS as u64) << 32),
    );
    wrmsr(msr::IA32_LSTAR, syscall_entry as *const u8 as u64);
    wrmsr(msr::IA32_FMASK, SYSCALL_RFLAGS_MASK);

    // RIP for compatibility mode. We don't support it for now.
    wrmsr(msr::IA32_CSTAR, 0);

    // Enable SYSCALL/SYSRET.
    wrmsr(msr::IA32_EFER, rdmsr(msr::IA32_EFER) | 1);
}
