// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-CPU interrupt flag (IF) transition recorder.
//!
//! Records every transition that changes the IF bit in RFLAGS:
//! CLI, STI, IRETQ, POPFQ, lock acquire/release.  When the NMI
//! watchdog fires on a stuck CPU, the ring buffer shows the exact
//! sequence that led to permanent IF=0.
//!
//! Each entry is 16 bytes: 8 (TSC) + 4 (source location hash) + 2 (event) + 2 (pad).
//! 256 entries per CPU = 4KB per CPU.  At 100Hz timer + lock ops, the
//! buffer covers ~1-2 seconds of history — enough to capture the
//! transition that never gets undone.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const RING_SIZE: usize = 256;
const MAX_CPUS: usize = 8;

/// Event types for IF transitions.
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum IfEvent {
    Cli          = 0,  // Explicit cli instruction
    Sti          = 1,  // Explicit sti instruction
    LockAcquire  = 2,  // SpinLock::lock() — saves RFLAGS + cli
    LockRelease  = 3,  // SpinLockGuard drop — restores RFLAGS
    IretqRing0   = 4,  // iretq returning to ring 0
    IretqRing3   = 5,  // iretq returning to ring 3
    SyscallEntry = 6,  // SYSCALL instruction (FMASK clears IF)
    Sysret       = 7,  // SYSRET returning to user
    IdleSti      = 8,  // idle loop's sti before hlt
    IdleCli      = 9,  // idle loop's cli after hlt
    SwitchSave   = 10, // do_switch_thread: prev context saved
    SwitchLoad   = 11, // do_switch_thread: next context loaded
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IfEntry {
    tsc: u64,
    /// Truncated address or identifier of the source location.
    source: u32,
    /// Event type.
    event: u8,
    /// Current IF state after this event (1=enabled, 0=disabled).
    if_after: u8,
    _pad: u16,
}

impl IfEntry {
    const EMPTY: Self = IfEntry { tsc: 0, source: 0, event: 0, if_after: 0, _pad: 0 };
}

/// Per-CPU ring buffer storage.
static mut RINGS: [[IfEntry; RING_SIZE]; MAX_CPUS] =
    [[IfEntry::EMPTY; RING_SIZE]; MAX_CPUS];

/// Per-CPU write index (wraps around RING_SIZE).
static IDX: [AtomicU32; MAX_CPUS] = {
    const Z: AtomicU32 = AtomicU32::new(0);
    [Z; MAX_CPUS]
};

/// Runtime enable flag.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable the IF tracer.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
    log::info!("if-trace: interrupt state tracker enabled");
}

/// Check if IF tracing is enabled.
#[inline(always)]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Record an IF transition event for the current CPU.
///
/// `source` is a truncated address or identifier (e.g., lock address & 0xFFFFFFFF).
/// `if_after` is true if IF=1 after this event.
#[inline(always)]
pub fn record(event: IfEvent, source: u32, if_after: bool) {
    if !is_enabled() {
        return;
    }
    let cpu = super::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    let idx = IDX[cpu].fetch_add(1, Ordering::Relaxed) as usize % RING_SIZE;
    // Safety: each CPU only writes to its own ring; no concurrent access.
    #[allow(unsafe_code)]
    unsafe {
        RINGS[cpu][idx] = IfEntry {
            tsc,
            source,
            event: event as u8,
            if_after: if_after as u8,
            _pad: 0,
        };
    }
}

/// Dump the IF transition history for a specific CPU.
/// Called from the NMI handler — must be lock-free.
pub fn dump(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let write_idx = IDX[cpu].load(Ordering::Relaxed) as usize;
    let count = write_idx.min(RING_SIZE);
    if count == 0 {
        log::warn!("  if-trace: no events recorded for CPU {}", cpu);
        return;
    }
    // Print the last N entries (most recent last).
    let start = if write_idx >= RING_SIZE { write_idx - RING_SIZE } else { 0 };
    // Limit to last 32 entries to avoid flooding serial.
    let display_start = if count > 32 { start + count - 32 } else { start };

    log::warn!("  if-trace: last {} events for CPU {} (of {} total):",
        count.min(32), cpu, write_idx);

    for i in display_start..start + count {
        let idx = i % RING_SIZE;
        #[allow(unsafe_code)]
        let entry = unsafe { &RINGS[cpu][idx] };
        let event_name = match entry.event {
            0 => "CLI",
            1 => "STI",
            2 => "LOCK_ACQ",
            3 => "LOCK_REL",
            4 => "IRETQ_R0",
            5 => "IRETQ_R3",
            6 => "SYSCALL",
            7 => "SYSRET",
            8 => "IDLE_STI",
            9 => "IDLE_CLI",
            10 => "SW_SAVE",
            11 => "SW_LOAD",
            _ => "???",
        };
        let if_str = if entry.if_after != 0 { "IF=1" } else { "IF=0" };
        log::warn!("    [{:>4}] tsc={} {:>10} src={:#010x} → {}",
            i - display_start, entry.tsc, event_name, entry.source, if_str);
    }
}
