// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Usercopy tracing and context tracking.
//!
//! Provides a lightweight "context tag" mechanism so that when a page fault
//! occurs during a usercopy operation, we can identify which kernel code
//! path was responsible.
//!
//! # Usage
//!
//! ```rust
//! // In an ioctl handler:
//! debug::usercopy::set_context("ioctl:TCGETS");
//! addr.write::<Termios>(&termios)?;
//! debug::usercopy::clear_context();
//!
//! // In signal delivery:
//! debug::usercopy::set_context("signal_stack:trampoline");
//! user_rsp.write_bytes(TRAMPOLINE)?;
//! debug::usercopy::clear_context();
//! ```
//!
//! When the page fault handler detects a fault in the usercopy region,
//! it reads the context tag to include in the `UsercopyFault` event.

use core::sync::atomic::{AtomicPtr, Ordering};

/// Global context tag for the current usercopy operation.
/// This is safe because usercopy cannot be preempted (interrupts are
/// serialized; the kernel is non-preemptive during usercopy).
static USERCOPY_CONTEXT: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());

/// Static empty string for when no context is set.
static EMPTY: &[u8] = b"\0";

/// Set the current usercopy context tag.
///
/// The tag must be a `&'static str` (string literal). It identifies
/// which kernel code path is performing the current usercopy.
#[inline]
pub fn set_context(tag: &'static str) {
    USERCOPY_CONTEXT.store(tag.as_ptr() as *mut u8, Ordering::Release);
}

/// Clear the usercopy context tag.
#[inline]
pub fn clear_context() {
    USERCOPY_CONTEXT.store(core::ptr::null_mut(), Ordering::Release);
}

/// Get the current usercopy context tag, or "" if none set.
pub fn get_context() -> &'static str {
    let ptr = USERCOPY_CONTEXT.load(Ordering::Acquire);
    if ptr.is_null() {
        return "";
    }
    // Safety: set_context only stores pointers from &'static str.
    // We need to find the length. Since these are always string literals,
    // we can scan for common terminators or use a fixed approach.
    // We use a simple bounded scan since these are short tags.
    let mut len = 0;
    #[allow(unsafe_code)]
    unsafe {
        while len < 128 {
            if *ptr.add(len) == 0 || *ptr.add(len) > 127 {
                break;
            }
            len += 1;
        }
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, len))
    }
}

/// Emit a usercopy trace event if the USERCOPY filter is enabled.
///
/// Called from `UserVAddr::write_bytes`, `read_bytes`, etc.
pub fn trace_usercopy(
    pid: i32,
    direction: &'static str,
    user_addr: usize,
    len: usize,
) {
    use super::emit;
    use super::event::DebugEvent;
    use super::filter::DebugFilter;

    if !emit::is_enabled(DebugFilter::USERCOPY) {
        return;
    }

    let ctx = get_context();
    emit::emit(DebugFilter::USERCOPY, &DebugEvent::Usercopy {
        pid,
        direction,
        user_addr,
        len,
        context: ctx,
    });
}
