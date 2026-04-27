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

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn cpu_have_feature(_num: u16) -> bool {
    true
}

ksym!(cpu_have_feature);
