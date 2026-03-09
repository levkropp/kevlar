// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Debug event emission to the serial output channel.
//!
//! Events are written to the kernel's debug printer (serial). The emitter
//! uses `try_lock` to avoid deadlocks when debug events fire inside locked
//! sections — events are silently dropped if the lock is contended.

use super::event::DebugEvent;
use super::filter::DebugFilter;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, Ordering};

/// Runtime debug filter — controls which event categories are emitted.
/// Default: nothing enabled (zero overhead unless explicitly turned on).
static DEBUG_FILTER: AtomicU32 = AtomicU32::new(0);

/// Debug event sequence number for ordering.
static DEBUG_SEQ: AtomicU32 = AtomicU32::new(0);

/// Lock-free guard to prevent reentrant debug emission (e.g. debug event
/// inside a debug event formatter). Uses an atomic flag instead of a lock.
static EMITTING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Set the runtime debug filter.
pub fn set_filter(filter: DebugFilter) {
    DEBUG_FILTER.store(filter.bits(), Ordering::Relaxed);
}

/// Get the current debug filter.
pub fn get_filter() -> DebugFilter {
    DebugFilter::from_bits_truncate(DEBUG_FILTER.load(Ordering::Relaxed))
}

/// Check if a given filter category is enabled.
#[inline]
pub fn is_enabled(category: DebugFilter) -> bool {
    DEBUG_FILTER.load(Ordering::Relaxed) & category.bits() != 0
}

/// A writer that emits to the kernel's debug printer with no allocation.
struct DebugWriter;

impl Write for DebugWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        kevlar_platform::print::get_debug_printer().print_bytes(s.as_bytes());
        Ok(())
    }
}

/// Emit a debug event to the serial output.
///
/// This is the single bottleneck for all debug output. It:
/// 1. Checks the filter (fast path: bit test on an atomic)
/// 2. Guards against reentrant emission
/// 3. Writes the JSONL line directly to serial (no heap allocation)
pub fn emit(category: DebugFilter, event: &DebugEvent<'_>) {
    if !is_enabled(category) {
        return;
    }

    // Prevent reentrant emission (e.g. page fault during debug emit).
    if EMITTING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    let _seq = DEBUG_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut w = DebugWriter;
    // Ignore write errors — serial output is best-effort for debug.
    let _ = event.write_jsonl(&mut w);

    EMITTING.store(false, Ordering::Release);
}

/// Parse a debug filter from a kernel command-line string.
///
/// Format: `debug=syscall,signal,fault,process,canary,memory`
/// Or: `debug=all` to enable everything.
/// Or: `debug=none` to disable everything (default).
pub fn parse_cmdline_filter(value: &str) -> DebugFilter {
    let mut filter = DebugFilter::empty();
    for token in value.split(',') {
        match token.trim() {
            "all" => return DebugFilter::all(),
            "none" => return DebugFilter::empty(),
            "syscall" | "syscalls" => filter |= DebugFilter::SYSCALL,
            "signal" | "signals" => filter |= DebugFilter::SIGNAL,
            "fault" | "faults" => filter |= DebugFilter::FAULT,
            "process" | "processes" => filter |= DebugFilter::PROCESS,
            "canary" => filter |= DebugFilter::CANARY,
            "memory" | "mem" => filter |= DebugFilter::MEMORY,
            "panic" => filter |= DebugFilter::PANIC,
            "usercopy" | "ucopy" => filter |= DebugFilter::USERCOPY,
            _ => {} // ignore unknown tokens
        }
    }
    filter
}
