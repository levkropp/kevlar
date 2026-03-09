// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Assembly-level usercopy trace ring buffer.
//!
//! The x86_64 `copy_to_user` / `copy_from_user` assembly writes the **actual**
//! register values (rdi=dst, rsi=src, rdx=len, return_addr) into a ring buffer
//! at entry, before any computation. This captures what the CPU truly executed,
//! not what the Rust caller thinks it passed.
//!
//! This is critical for diagnosing bugs where rdx (len) is corrupted between
//! the Rust call site and the assembly entry — e.g., an interrupt handler that
//! clobbers rdx, or a calling-convention mismatch.
//!
//! # Usage
//!
//! ```rust
//! // Enable tracing (from debug init):
//! usercopy_trace::enable();
//!
//! // After detecting a canary corruption or fault, dump the ring buffer:
//! let entries = usercopy_trace::snapshot();
//! for e in &entries {
//!     println!("copy: dst={:#x} src={:#x} len={} ret={:#x}", e.dst, e.src, e.len, e.ret_addr);
//! }
//! ```

/// A single usercopy trace entry, as written by the assembly probe.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct UcopyTraceEntry {
    pub dst: usize,
    pub src: usize,
    pub len: usize,
    pub ret_addr: usize,
}

const TRACE_ENTRIES: usize = 32;

#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
unsafe extern "C" {
    static mut ucopy_trace_buf: [UcopyTraceEntry; TRACE_ENTRIES];
    static mut ucopy_trace_idx: u64;
    static mut ucopy_trace_on: u64;
}

/// Enable the assembly-level usercopy trace probe.
///
/// Once enabled, every `copy_to_user` / `copy_from_user` call records its
/// register values into the ring buffer. The overhead is ~15 cycles per call
/// (a few stores to cached BSS memory).
#[cfg(target_arch = "x86_64")]
pub fn enable() {
    #[allow(unsafe_code)]
    unsafe {
        core::ptr::write_volatile(&raw mut ucopy_trace_on, 1);
    }
}

/// Disable the assembly-level usercopy trace probe.
#[cfg(target_arch = "x86_64")]
pub fn disable() {
    #[allow(unsafe_code)]
    unsafe {
        core::ptr::write_volatile(&raw mut ucopy_trace_on, 0);
    }
}

/// Check if the trace probe is enabled.
#[cfg(target_arch = "x86_64")]
pub fn is_enabled() -> bool {
    #[allow(unsafe_code)]
    unsafe {
        core::ptr::read_volatile(&raw const ucopy_trace_on) != 0
    }
}

/// Take a snapshot of the ring buffer, returning entries in chronological order
/// (oldest first). Returns up to TRACE_ENTRIES entries.
#[cfg(target_arch = "x86_64")]
pub fn snapshot() -> arrayvec::ArrayVec<UcopyTraceEntry, TRACE_ENTRIES> {
    #[allow(unsafe_code)]
    let (buf_copy, idx) = unsafe {
        let idx = core::ptr::read_volatile(&raw const ucopy_trace_idx) as usize;
        let mut buf = [UcopyTraceEntry { dst: 0, src: 0, len: 0, ret_addr: 0 }; TRACE_ENTRIES];
        core::ptr::copy_nonoverlapping(
            (&raw const ucopy_trace_buf) as *const UcopyTraceEntry,
            buf.as_mut_ptr(),
            TRACE_ENTRIES,
        );
        (buf, idx)
    };

    let mut result = arrayvec::ArrayVec::new();
    let total = idx.min(TRACE_ENTRIES);

    // Start from the oldest entry in the ring buffer.
    let start = if idx >= TRACE_ENTRIES { idx % TRACE_ENTRIES } else { 0 };
    for i in 0..total {
        let entry = buf_copy[(start + i) % TRACE_ENTRIES];
        // Skip zeroed entries (unfilled slots).
        if entry.dst == 0 && entry.src == 0 && entry.len == 0 {
            continue;
        }
        result.push(entry);
    }
    result
}

/// Return the total number of usercopy calls recorded since enable().
#[cfg(target_arch = "x86_64")]
pub fn call_count() -> u64 {
    #[allow(unsafe_code)]
    unsafe {
        core::ptr::read_volatile(&raw const ucopy_trace_idx)
    }
}

// ── ARM64 stubs ──
// ARM64 usercopy doesn't have the trace probe yet.

#[cfg(not(target_arch = "x86_64"))]
pub fn enable() {}

#[cfg(not(target_arch = "x86_64"))]
pub fn disable() {}

#[cfg(not(target_arch = "x86_64"))]
pub fn is_enabled() -> bool {
    false
}

#[cfg(not(target_arch = "x86_64"))]
pub fn snapshot() -> arrayvec::ArrayVec<UcopyTraceEntry, TRACE_ENTRIES> {
    arrayvec::ArrayVec::new()
}

#[cfg(not(target_arch = "x86_64"))]
pub fn call_count() -> u64 {
    0
}
