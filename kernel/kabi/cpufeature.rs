// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! arm64 cpufeature primitive shim.
//!
//! Linux's `cpu_have_feature(num)` reads a per-cpu bitmap of
//! detected hardware capabilities (NEON, FP, atomics, BTI, etc.)
//! and returns whether the running CPU implements a given feature.
//!
//! K10 stubs to `true` — Kevlar runs on QEMU virt arm64 with KVM
//! pass-through, where all common features (NEON, FP, atomics,
//! crc32, sha) are present.  Drivers that branch on this get the
//! "yes, you can use it" path.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn cpu_have_feature(_num: u16) -> bool {
    true
}

ksym!(cpu_have_feature);

/// arm64 alternative-instruction patcher.  Linux uses this to patch
/// in NEON-enabled / errata-workaround variants of code at boot.
/// K11: leave the original instructions (compiled-in `nop`
/// placeholders).  If a module's correctness depends on the patch
/// firing, we'll see misbehavior at K12+ and revisit.
#[unsafe(no_mangle)]
pub extern "C" fn alt_cb_patch_nops(
    _alt_kind: *const c_void,
    _orig_ptr: *mut u32,
    _updptr: *mut u32,
    _nr_inst: i32,
) {
}

ksym!(alt_cb_patch_nops);
