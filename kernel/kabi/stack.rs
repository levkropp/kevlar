// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Stack-protector (`-fstack-protector`) compiler-injected helpers
//! that Ubuntu's modules reference.
//!
//! `__stack_chk_guard` is a static u64 the prologue copies into a
//! local; the epilogue compares the local against the global and
//! calls `__stack_chk_fail()` on mismatch.  Real Linux randomizes
//! the guard at boot; K11 uses a fixed sentinel — sufficient as
//! long as no module corrupts its own stack between prologue and
//! epilogue (in which case the canary catches it and we panic).

use crate::{ksym, ksym_static};

#[unsafe(no_mangle)]
pub static __stack_chk_guard: u64 = 0xDEAD_BEEF_CAFE_BABE;

#[unsafe(no_mangle)]
pub extern "C" fn __stack_chk_fail() -> ! {
    panic!("kabi: __stack_chk_fail — module canary corruption");
}

ksym_static!(__stack_chk_guard);
ksym!(__stack_chk_fail);
