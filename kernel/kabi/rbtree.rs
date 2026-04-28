// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux red-black tree (`rb_tree`) shim.
//!
//! `lib/rbtree.c` exports a small set of operations that DRM and FS
//! subsystems use as ordered set/map primitives.  K13 stubs them to
//! no-ops/null because `drm_buddy.ko` only references the symbols
//! at link time — no callers fire during load (drm_buddy has no
//! `init_module`).
//!
//! K14+: when a DRM driver actually exercises the buddy allocator
//! we replace these with real RB-tree code (or use a Rust BTreeMap
//! with the same field-offset layout for the `struct rb_node`
//! caller objects embed).

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn rb_erase(_node: *mut c_void, _root: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn rb_insert_color(_node: *mut c_void, _root: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn rb_first_postorder(_root: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn rb_next_postorder(_node: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn rb_prev(_node: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

ksym!(rb_erase);
ksym!(rb_insert_color);
ksym!(rb_first_postorder);
ksym!(rb_next_postorder);
ksym!(rb_prev);
