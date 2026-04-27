// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Scheduler / task primitives exposed to K2 modules.
//!
//! `kabi_current()` returns an opaque `*mut Process` that K2 modules
//! pass back to accessor shims (`kabi_current_pid`, etc.).  Modules
//! do not dereference Process fields directly — that requires
//! Linux-shape struct-layout faithfulness, deferred to K3+.

use core::ffi::{c_char, c_void};

use crate::ksym;
use crate::process::{current_process, switch};

#[unsafe(no_mangle)]
pub extern "C" fn kabi_current() -> *mut c_void {
    let p = current_process();
    alloc::sync::Arc::as_ptr(p) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn kabi_current_pid() -> i32 {
    current_process().pid().as_i32()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn kabi_current_comm(buf: *mut c_char, len: usize) {
    if buf.is_null() || len == 0 {
        return;
    }
    let p = current_process();
    let bytes = p.get_comm();
    let n = bytes.len().min(len - 1);
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, n);
        *buf.add(n) = 0;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn msleep(ms: u32) {
    crate::timer::_sleep_ms(ms as usize);
}

/// `schedule()` voluntarily yields.  Linux modules call this in
/// busy-wait fallbacks; ours just calls `switch()`.
#[unsafe(no_mangle)]
pub extern "C" fn schedule() {
    let _ = switch();
}

#[unsafe(no_mangle)]
pub extern "C" fn cond_resched() -> i32 {
    let _ = switch();
    0
}

/// Linux's `schedule_timeout(ticks)` sleeps up to `ticks` jiffies,
/// returning the remaining jiffies on early wakeup.  K2 honors the
/// timeout (TICK_HZ = 100, so 1 jiffy = 10 ms) and always returns
/// 0 (no early-wake support).
#[unsafe(no_mangle)]
pub extern "C" fn schedule_timeout(ticks: i64) -> i64 {
    if ticks <= 0 {
        return 0;
    }
    let ms = (ticks as usize).saturating_mul(10);
    crate::timer::_sleep_ms(ms);
    0
}

ksym!(kabi_current);
ksym!(kabi_current_pid);
ksym!(kabi_current_comm);
ksym!(msleep);
ksym!(schedule);
ksym!(cond_resched);
ksym!(schedule_timeout);
