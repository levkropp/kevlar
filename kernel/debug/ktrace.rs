// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ktrace — High-bandwidth binary kernel tracing system.
//!
//! Records fixed-size 32-byte events to per-CPU lock-free ring buffers, then
//! dumps via QEMU debugcon (port 0xe9, ~5 MB/s on KVM — 350x faster than
//! serial). Host-side `tools/ktrace-decode.py` decodes the binary dump into
//! text timelines and Perfetto JSON for visualization.
//!
//! # Compile-time gating
//!
//! All tracing compiles to nothing without the `ktrace` feature flag.
//! With the feature enabled but runtime-disabled: one atomic load per trace
//! site (~1ns). When enabled: ~30ns per event (rdtsc + atomic store).
//!
//! # Usage
//!
//! ```rust
//! crate::debug::ktrace::trace(event::SYSCALL_ENTER, nr as u32, a1_lo, a1_hi, a2_lo, a2_hi);
//! ```
//!
//! Dump on PID 1 exit, panic, or via `debug=ktrace-dump`.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ── Configuration ───────────────────────────────────────────────────────

/// Whether ktrace recording is active at runtime.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Ring buffer entries per CPU.
const RING_SIZE: usize = 8192;

/// Maximum supported CPUs.
const MAX_CPUS: usize = 8;

// ── TraceRecord ─────────────────────────────────────────────────────────

/// A single trace event: 32 bytes, cache-line friendly.
#[derive(Copy, Clone)]
#[repr(C, align(32))]
pub struct TraceRecord {
    /// Raw TSC timestamp.
    pub tsc: u64,
    /// Packed header: [event_type:10 | cpu:3 | pid_idx:11 | flags:8]
    pub header: u32,
    /// Event-specific payload (5 × u32 = 20 bytes).
    pub data: [u32; 5],
}

impl TraceRecord {
    const ZERO: TraceRecord = TraceRecord {
        tsc: 0,
        header: 0,
        data: [0; 5],
    };

    /// Pack the header from components.
    #[inline(always)]
    pub fn pack_header(event_type: u16, cpu: u8, pid: u16, flags: u8) -> u32 {
        ((event_type as u32) & 0x3FF)
            | (((cpu as u32) & 0x7) << 10)
            | (((pid as u32) & 0x7FF) << 13)
            | ((flags as u32) << 24)
    }

    /// Extract event type from header (bits 0-9).
    pub fn event_type(&self) -> u16 {
        (self.header & 0x3FF) as u16
    }

    /// Extract CPU id from header (bits 10-12).
    pub fn cpu(&self) -> u8 {
        ((self.header >> 10) & 0x7) as u8
    }

    /// Extract PID index from header (bits 13-23).
    pub fn pid_idx(&self) -> u16 {
        ((self.header >> 13) & 0x7FF) as u16
    }

    /// Extract flags from header (bits 24-31).
    pub fn flags(&self) -> u8 {
        (self.header >> 24) as u8
    }
}

// ── Event type constants ────────────────────────────────────────────────

pub mod event {
    // Core
    pub const SYSCALL_ENTER: u16 = 0;
    pub const SYSCALL_EXIT: u16 = 1;
    pub const CTX_SWITCH: u16 = 5;

    // Scheduler (70-79)
    pub const WAITQ_SLEEP: u16 = 70;
    pub const WAITQ_WAKE: u16 = 71;

    // Memory management (10-19)
    pub const PAGE_FAULT: u16 = 10;

    // Network (193-210)
    pub const NET_CONNECT: u16 = 193;
    pub const NET_SEND: u16 = 197;
    pub const NET_RECV: u16 = 198;
    pub const NET_POLL: u16 = 199;
    pub const NET_RX_PACKET: u16 = 201;
    pub const NET_TX_PACKET: u16 = 202;
    pub const NET_TCP_STATE: u16 = 203;
    pub const NET_DNS_QUERY: u16 = 204;

    /// Map event type to name.
    pub fn name(ty: u16) -> &'static str {
        match ty {
            SYSCALL_ENTER => "SYSCALL_ENTER",
            SYSCALL_EXIT => "SYSCALL_EXIT",
            CTX_SWITCH => "CTX_SWITCH",
            WAITQ_SLEEP => "WAITQ_SLEEP",
            WAITQ_WAKE => "WAITQ_WAKE",
            PAGE_FAULT => "PAGE_FAULT",
            NET_CONNECT => "NET_CONNECT",
            NET_SEND => "NET_SEND",
            NET_RECV => "NET_RECV",
            NET_POLL => "NET_POLL",
            NET_RX_PACKET => "NET_RX_PACKET",
            NET_TX_PACKET => "NET_TX_PACKET",
            NET_TCP_STATE => "NET_TCP_STATE",
            NET_DNS_QUERY => "NET_DNS_QUERY",
            _ => "UNKNOWN",
        }
    }
}

// ── Per-CPU ring buffer ─────────────────────────────────────────────────

/// Per-CPU trace ring. Aligned to avoid false sharing.
#[repr(align(64))]
struct CpuRing {
    ring: [TraceRecord; RING_SIZE],
    write_idx: AtomicUsize,
}

impl CpuRing {
    const fn new() -> CpuRing {
        CpuRing {
            ring: [TraceRecord::ZERO; RING_SIZE],
            write_idx: AtomicUsize::new(0),
        }
    }
}

/// All per-CPU ring buffers.
#[allow(static_mut_refs)]
static mut CPU_RINGS: [CpuRing; MAX_CPUS] = [
    CpuRing::new(), CpuRing::new(), CpuRing::new(), CpuRing::new(),
    CpuRing::new(), CpuRing::new(), CpuRing::new(), CpuRing::new(),
];

// ── Public API ──────────────────────────────────────────────────────────

/// Enable ktrace recording.
pub fn enable() {
    ENABLED.store(true, Ordering::Release);
}

/// Check if ktrace is enabled (one atomic load — fast path).
#[inline(always)]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Record a trace event into the current CPU's ring buffer.
///
/// # Arguments
/// - `event_type`: Event type constant from `event` module
/// - `d0..d4`: Event-specific u32 payload fields
#[inline(always)]
#[allow(unsafe_code)]
pub fn record(event_type: u16, d0: u32, d1: u32, d2: u32, d3: u32, d4: u32) {
    if !is_enabled() {
        return;
    }

    let cpu = kevlar_platform::arch::cpu_id() as usize;
    let tsc = kevlar_platform::arch::read_clock_counter();

    // Get current PID (truncated to 11 bits = 0..2047).
    // Uses try_current_pid() to avoid panic during early boot before the
    // process subsystem is initialized.
    let pid = crate::process::try_current_pid() as u16;

    let header = TraceRecord::pack_header(event_type, cpu as u8, pid, 0);

    // SAFETY: Only this CPU writes to its ring (single-producer).
    unsafe {
        let ring = &mut CPU_RINGS[cpu % MAX_CPUS];
        let idx = ring.write_idx.fetch_add(1, Ordering::Relaxed) % RING_SIZE;
        let ptr = &ring.ring as *const _ as *mut [TraceRecord; RING_SIZE];
        (*ptr)[idx] = TraceRecord {
            tsc,
            header,
            data: [d0, d1, d2, d3, d4],
        };
    }
}

// ── Dump protocol ───────────────────────────────────────────────────────

/// Binary dump header: 64 bytes.
#[repr(C)]
struct DumpHeader {
    magic: [u8; 4],       // "KTRX"
    version: u32,         // 1
    tsc_freq_hz: u64,
    num_cpus: u32,
    ring_size: u32,       // 8192
    entry_size: u32,      // 32
    flags: u32,
    _reserved: [u8; 32],
}

/// Dump all per-CPU ring buffers via debugcon.
///
/// Called on PID 1 exit, panic, or when `debug=ktrace-dump` is active.
/// Writes a binary header followed by each CPU's ring data.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
pub fn dump() {
    use kevlar_platform::arch::tsc;

    let ncpus = kevlar_platform::arch::num_online_cpus().min(MAX_CPUS as u32) as usize;
    let tsc_freq = tsc::frequency_hz();

    // Write header.
    let header = DumpHeader {
        magic: *b"KTRX",
        version: 1,
        tsc_freq_hz: tsc_freq,
        num_cpus: ncpus as u32,
        ring_size: RING_SIZE as u32,
        entry_size: 32,
        flags: 0,
        _reserved: [0; 32],
    };

    let header_bytes = unsafe {
        core::slice::from_raw_parts(
            &header as *const DumpHeader as *const u8,
            core::mem::size_of::<DumpHeader>(),
        )
    };
    kevlar_platform::debugcon::write_bytes(header_bytes);

    // Write per-CPU rings.
    for cpu in 0..ncpus {
        let ring = unsafe { &CPU_RINGS[cpu] };
        let ring_bytes = unsafe {
            core::slice::from_raw_parts(
                ring.ring.as_ptr() as *const u8,
                RING_SIZE * 32,
            )
        };
        kevlar_platform::debugcon::write_bytes(ring_bytes);
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn dump() {
    // ARM64: fall back to serial dump (future: MMIO debugcon).
    warn!("ktrace: dump not implemented for this architecture");
}

/// Dump a text summary to serial (for quick inspection without host tools).
pub fn dump_summary() {
    let ncpus = kevlar_platform::arch::num_online_cpus().min(MAX_CPUS as u32) as usize;
    let mut total_events = 0usize;

    for cpu in 0..ncpus {
        #[allow(unsafe_code)]
        let ring = unsafe { &CPU_RINGS[cpu] };
        let write_idx = ring.write_idx.load(Ordering::Relaxed);
        total_events += write_idx;

        // Count events by type.
        let count = write_idx.min(RING_SIZE);
        let start = if write_idx >= RING_SIZE { write_idx - RING_SIZE } else { 0 };

        let mut syscall_enter = 0u32;
        let mut syscall_exit = 0u32;
        let mut ctx_switch = 0u32;
        let mut waitq_sleep = 0u32;
        let mut waitq_wake = 0u32;
        let mut net_events = 0u32;
        let mut page_faults = 0u32;

        for i in start..write_idx {
            let entry = ring.ring[i % RING_SIZE];
            if entry.tsc == 0 { continue; }
            match entry.event_type() {
                event::SYSCALL_ENTER => syscall_enter += 1,
                event::SYSCALL_EXIT => syscall_exit += 1,
                event::CTX_SWITCH => ctx_switch += 1,
                event::WAITQ_SLEEP => waitq_sleep += 1,
                event::WAITQ_WAKE => waitq_wake += 1,
                event::PAGE_FAULT => page_faults += 1,
                193..=210 => net_events += 1,
                _ => {}
            }
        }

        info!(
            "ktrace CPU{}: {} events (syscall={}/{} ctx={} waitq={}/{} net={} pf={})",
            cpu, count, syscall_enter, syscall_exit, ctx_switch,
            waitq_sleep, waitq_wake, net_events, page_faults,
        );
    }

    info!("ktrace: {} total events across {} CPUs", total_events, ncpus);
}

// ── Convenience inline ──────────────────────────────────────────────────

/// Convenience wrapper with automatic u32 casts.
/// Use this from trace instrumentation sites. Compiles to a single
/// `is_enabled()` check + ring buffer write when the ktrace feature is on.
#[inline(always)]
pub fn trace(event_type: u16, d0: u32, d1: u32, d2: u32, d3: u32, d4: u32) {
    record(event_type, d0, d1, d2, d3, d4);
}
