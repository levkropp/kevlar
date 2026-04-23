// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//!
//! Per-CPU lock-free flight recorder.
//!
//! Maintains a small circular buffer of the most recent events on each CPU.
//! Designed to be dumped during a kernel panic (after all other CPUs are
//! halted) to show a cross-CPU timeline of what led to the crash.
//!
//! # Safety
//!
//! **Write path:** Only CPU `n` writes to `RINGS[n]`.  Since each CPU owns
//! its slice exclusively, no synchronization is needed for the write itself.
//! The index (`IDX[n]`) uses a relaxed atomic increment so the write is
//! visible to other CPUs eventually, but exact visibility is not required —
//! the dump only runs after all peers are halted.
//!
//! **Dump path:** Called from the panic handler, which has already broadcast
//! a halt IPI.  By the time `dump()` runs, all other CPUs are spinning in
//! `hlt` and will not write to their rings.  Reading `RINGS` without locks
//! is therefore safe.
//!
//! # Entry layout (32 bytes = 4 × u64)
//!
//! ```
//!  [0] tsc         : u64   — raw TSC timestamp
//!  [1] kind:u8 | cpu:u8 | _pad:u16 | data0:u32  — packed descriptor
//!  [2] data1       : u64
//!  [3] data2       : u64
//! ```

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ── Constants ───────────────────────────────────────────────────────────────

pub const MAX_CPUS:  usize = 8;
/// Ring size per CPU — increased from 64 to 256 for better crash coverage.
/// 256 entries × 32 bytes = 8KB per CPU, 64KB total for 8 CPUs.
pub const RING_SIZE: usize = 256;

/// Event kind codes.
pub mod kind {
    pub const CTX_SWITCH:      u8 = 1;
    pub const TLB_SEND:        u8 = 2;
    pub const TLB_RECV:        u8 = 3;
    pub const MUNMAP:          u8 = 4;
    pub const MMAP_FAULT:      u8 = 5;
    pub const PREEMPT:         u8 = 6;
    pub const SYSCALL_IN:      u8 = 7;
    pub const SYSCALL_OUT:     u8 = 8;
    pub const IDLE_ENTER:      u8 = 9;
    pub const IDLE_EXIT:       u8 = 10;
    pub const SIGNAL:          u8 = 11;
    // Milestone T diagnostic event kinds:
    pub const NMI_WATCHDOG:    u8 = 12;
    pub const LOCKDEP_ACQUIRE: u8 = 13;
    pub const LOCKDEP_RELEASE: u8 = 14;
    pub const IF_TRANSITION:   u8 = 15;
    pub const GUARD_PAGE_HIT:  u8 = 16;
    // Allocator / VM instrumentation (ARM64 contract-test debugging).
    pub const PAGE_ALLOC:      u8 = 17;
    pub const PAGE_FREE:       u8 = 18;
    pub const UNMAP_USER:      u8 = 19;
    pub const MAP_USER:        u8 = 20;
}

/// Global monotonic sequence counter.  Stamped on every event to allow
/// merging per-CPU buffers into a causally-ordered timeline (TSC can
/// drift across CPUs; sequence numbers cannot).
static GLOBAL_SEQ: AtomicU64 = AtomicU64::new(0);

// ── Storage ─────────────────────────────────────────────────────────────────

/// Raw ring buffer storage.  Indexed as `RINGS[cpu][entry_index][word]`.
///
/// Safety invariant: `RINGS[n]` is only written by CPU `n`.
// Each element is [[u64; 4]; RING_SIZE]. We need 8 of them.
// `static mut` is sound here because:
//   - writes are per-CPU (no concurrent writers for the same slice)
//   - reads (dump path) happen only after other CPUs are halted
#[allow(static_mut_refs)]
static mut RINGS: [[[u64; 4]; RING_SIZE]; MAX_CPUS] =
    [[[0u64; 4]; RING_SIZE]; MAX_CPUS];

/// Next-write index per CPU.  Wraps modulo RING_SIZE.
static IDX: [AtomicUsize; MAX_CPUS] = [
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
    AtomicUsize::new(0), AtomicUsize::new(0),
];

// ── Write path ──────────────────────────────────────────────────────────────

/// Record an event in the current CPU's flight-recorder ring buffer.
///
/// This is the hot-path entry point.  Keep it small and branch-free.
#[inline(always)]
pub fn record(kind: u8, data0: u32, data1: u64, data2: u64) {
    let cpu = crate::arch::cpu_id() as usize % MAX_CPUS;
    let raw_idx = IDX[cpu].fetch_add(1, Ordering::Relaxed);
    let idx = raw_idx % RING_SIZE;
    let tsc = crate::arch::read_clock_counter();
    // Global sequence number for causal ordering across CPUs.
    let seq = GLOBAL_SEQ.fetch_add(1, Ordering::Relaxed);
    // Safety: only CPU `cpu` writes to RINGS[cpu].
    unsafe {
        let slot = &mut RINGS[cpu][idx];
        slot[0] = tsc;
        // Pack: kind:8 | cpu:8 | seq_lo:16 | data0:32
        slot[1] = ((kind  as u64) << 56)
                | ((cpu   as u64) << 48)
                | (((seq & 0xFFFF) as u64) << 32)
                | (data0  as u64);
        slot[2] = data1;
        slot[3] = data2;
    }
}

// ── Dump path ───────────────────────────────────────────────────────────────

fn kind_name(k: u8) -> &'static str {
    match k {
        kind::CTX_SWITCH      => "CTX_SWITCH ",
        kind::TLB_SEND        => "TLB_SEND   ",
        kind::TLB_RECV        => "TLB_RECV   ",
        kind::MUNMAP          => "MUNMAP     ",
        kind::MMAP_FAULT      => "MMAP_FAULT ",
        kind::PREEMPT         => "PREEMPT    ",
        kind::SYSCALL_IN      => "SYSCALL_IN ",
        kind::SYSCALL_OUT     => "SYSCALL_OUT",
        kind::IDLE_ENTER      => "IDLE_ENTER ",
        kind::IDLE_EXIT       => "IDLE_EXIT  ",
        kind::SIGNAL          => "SIGNAL     ",
        kind::NMI_WATCHDOG    => "NMI_WDOG   ",
        kind::LOCKDEP_ACQUIRE => "LOCKDEP_ACQ",
        kind::LOCKDEP_RELEASE => "LOCKDEP_REL",
        kind::IF_TRANSITION   => "IF_TRANS   ",
        kind::GUARD_PAGE_HIT  => "GUARD_HIT  ",
        kind::PAGE_ALLOC      => "PAGE_ALLOC ",
        kind::PAGE_FREE       => "PAGE_FREE  ",
        kind::UNMAP_USER      => "UNMAP_USER ",
        kind::MAP_USER        => "MAP_USER   ",
        _                     => "???        ",
    }
}

/// Decode and print the data fields for each event kind.
fn print_event_detail(kind: u8, cpu: usize, data0: u32, data1: u64, data2: u64) {
    match kind {
        kind::CTX_SWITCH => {
            warn!("  CPU={} CTX_SWITCH  from_pid={} to_pid={}",
                cpu, data0, data1);
        }
        kind::TLB_SEND => {
            warn!("  CPU={} TLB_SEND    target_mask={:#x} vaddr={:#x} pages={}",
                cpu, data0, data1, data2);
        }
        kind::TLB_RECV => {
            if data1 == 0 {
                warn!("  CPU={} TLB_RECV    vaddr=0 (full CR3 reload)", cpu);
            } else {
                warn!("  CPU={} TLB_RECV    vaddr={:#x} (invlpg)", cpu, data1);
            }
        }
        kind::MUNMAP => {
            warn!("  CPU={} MUNMAP      pid={} addr={:#x} len={:#x}",
                cpu, data0, data1, data2);
        }
        kind::MMAP_FAULT => {
            warn!("  CPU={} MMAP_FAULT  pid={} fault_addr={:#x}",
                cpu, data0, data1);
        }
        kind::PREEMPT => {
            warn!("  CPU={} PREEMPT     pid={}", cpu, data0);
        }
        kind::SYSCALL_IN => {
            warn!("  CPU={} SYSCALL_IN  nr={} arg0={:#x}",
                cpu, data0, data1);
        }
        kind::SYSCALL_OUT => {
            let ret = data1 as i64;
            warn!("  CPU={} SYSCALL_OUT nr={} ret={}",
                cpu, data0, ret);
        }
        kind::IDLE_ENTER => {
            warn!("  CPU={} IDLE_ENTER", cpu);
        }
        kind::IDLE_EXIT => {
            warn!("  CPU={} IDLE_EXIT   vec={:#x}", cpu, data0);
        }
        kind::SIGNAL => {
            warn!("  CPU={} SIGNAL      pid={} sig={}", cpu, data0, data1);
        }
        kind::NMI_WATCHDOG => {
            warn!("  CPU={} NMI_WDOG    rip={:#x} rflags={:#x}",
                cpu, data1, data2);
        }
        kind::LOCKDEP_ACQUIRE => {
            warn!("  CPU={} LOCKDEP_ACQ lock_addr={:#x} rank={}",
                cpu, data1, data0);
        }
        kind::LOCKDEP_RELEASE => {
            warn!("  CPU={} LOCKDEP_REL lock_addr={:#x}",
                cpu, data1);
        }
        kind::IF_TRANSITION => {
            let if_after = if data2 != 0 { "IF=1" } else { "IF=0" };
            warn!("  CPU={} IF_TRANS    event={} src={:#x} → {}",
                cpu, data0, data1, if_after);
        }
        kind::GUARD_PAGE_HIT => {
            warn!("  CPU={} GUARD_HIT   fault_addr={:#x}",
                cpu, data1);
        }
        kind::PAGE_ALLOC => {
            warn!("  CPU={} PAGE_ALLOC  paddr={:#x} num_pages={} site={:#x}",
                cpu, data1, data2, data0);
        }
        kind::PAGE_FREE => {
            warn!("  CPU={} PAGE_FREE   paddr={:#x} num_pages={} site={:#x}",
                cpu, data1, data2, data0);
        }
        kind::UNMAP_USER => {
            warn!("  CPU={} UNMAP_USER  pid={} vaddr={:#x} paddr={:#x}",
                cpu, data0, data1, data2);
        }
        kind::MAP_USER => {
            warn!("  CPU={} MAP_USER    pid={} vaddr={:#x} paddr={:#x}",
                cpu, data0, data1, data2);
        }
        _ => {
            warn!("  CPU={} kind={} data0={:#x} data1={:#x} data2={:#x}",
                cpu, kind, data0, data1, data2);
        }
    }
    let _ = kind_name(kind); // suppress unused warning
}

/// A decoded flight recorder entry for sorting.
struct DecodedEntry {
    tsc:    u64,
    seq_lo: u16,
    cpu:    u8,
    kind:   u8,
    data0:  u32,
    data1:  u64,
    data2:  u64,
}

/// Dump all flight recorder ring buffers in TSC order.
///
/// # Safety
///
/// Must only be called from the panic handler after all peer CPUs have been
/// halted (via broadcast halt IPI).  Reading `RINGS` without locks is safe
/// at that point.
pub fn dump() {
    warn!("[FLIGHT RECORDER — last {} events per CPU, sorted by TSC]",
        RING_SIZE);

    // Collect all non-zero entries into a heap-allocated buffer.
    // MAX_CPUS * RING_SIZE = 8 * 256 = 2048 entries × ~40 B = ~80 KB —
    // larger than a 16 KB kernel stack, so must NOT be on the stack.
    use alloc::vec::Vec;
    let mut entries: Vec<DecodedEntry> = Vec::with_capacity(MAX_CPUS * RING_SIZE);
    let mut count = 0usize;

    let cur_cpu = crate::arch::cpu_id() as usize % MAX_CPUS;

    for cpu in 0..MAX_CPUS {
        let entries_written = IDX[cpu].load(Ordering::Relaxed);
        if entries_written == 0 {
            continue;
        }

        // Read from ring in chronological order (oldest first).
        let num = entries_written.min(RING_SIZE);
        let start = if entries_written > RING_SIZE {
            entries_written % RING_SIZE  // oldest slot
        } else {
            0
        };

        for i in 0..num {
            let idx = (start + i) % RING_SIZE;
            // Safety: either cpu == cur_cpu (we own it) or all peer CPUs
            // have been halted before this function is called.
            let slot = unsafe { &RINGS[cpu][idx] };
            let tsc        = slot[0];
            let descriptor = slot[1];
            let data1      = slot[2];
            let data2      = slot[3];

            if tsc == 0 {
                continue; // unwritten slot
            }

            let kind   = ((descriptor >> 56) & 0xff) as u8;
            let _ecpu  = ((descriptor >> 48) & 0xff) as u8;
            let seq_lo = ((descriptor >> 32) & 0xffff) as u16;
            let data0  = (descriptor & 0xffff_ffff) as u32;

            if count < entries.capacity() {
                entries.push(DecodedEntry { tsc, seq_lo, cpu: cpu as u8, kind, data0, data1, data2 });
                count += 1;
            }
        }

        let _ = cur_cpu; // suppress unused warning
    }

    if count == 0 {
        warn!("  (no events recorded)");
        return;
    }

    // Simple insertion sort by TSC (count ≤ 512, acceptable in O(n²)).
    for i in 1..count {
        let mut j = i;
        while j > 0 && entries[j].tsc < entries[j - 1].tsc {
            entries.swap(j, j - 1);
            j -= 1;
        }
    }

    // Use the first entry's TSC as the base for relative timestamps.
    let base_tsc = entries[0].tsc;
    // Approximate: use 1 tick ≈ 1 ns at ~1GHz to avoid needing tsc_freq here.
    // For relative display, raw TSC delta is informative enough.

    warn!("  (base TSC={:#x}, showing {} events, seq=global ordering)", base_tsc, count);
    for e in &entries[..count] {
        let delta = e.tsc.saturating_sub(base_tsc);
        warn!("  +{:>8} ticks  seq={:>5}  CPU={}  {}", delta, e.seq_lo, e.cpu,
            kind_name(e.kind));
        print_event_detail(e.kind, e.cpu as usize, e.data0, e.data1, e.data2);
    }
}
