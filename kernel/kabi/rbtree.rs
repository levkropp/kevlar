// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux red-black tree (`rb_tree`) shim.
//!
//! `lib/rbtree.c` exports a small set of operations that fs and DRM
//! subsystems use as ordered set/map primitives.  Phase 13 v3 made
//! `rb_next` and `rb_prev` real — ext4_dx_readdir's outer loop walks
//! its dir-entry rb_tree via `rb_next` and was emitting only one
//! entry because the stub returned NULL.  `rb_insert_color` stays
//! a no-op (tree isn't balanced, but rb_link_node is inlined and
//! does set parents/children correctly, which is enough for
//! in-order traversal).
//!
//! struct rb_node layout (include/linux/rbtree_types.h):
//!   +0    unsigned long __rb_parent_color  (low bit = color, top
//!         bits = parent pointer)
//!   +8    struct rb_node *rb_right
//!   +16   struct rb_node *rb_left
//! Total: 24 bytes, aligned to 8.

use core::ffi::c_void;

use crate::ksym;

const RB_PARENT_COLOR_OFF: usize = 0;
const RB_RIGHT_OFF: usize = 8;
const RB_LEFT_OFF: usize = 16;

#[inline(always)]
unsafe fn rb_parent(node: *const c_void) -> *mut c_void {
    let pc: usize = unsafe {
        *(node as *const u8).add(RB_PARENT_COLOR_OFF).cast::<usize>()
    };
    // Low bit is color; mask off.  All-zero high bits = no parent
    // (root).
    (pc & !1usize) as *mut c_void
}

#[inline(always)]
unsafe fn rb_left(node: *const c_void) -> *mut c_void {
    unsafe {
        *(node as *const u8).add(RB_LEFT_OFF).cast::<*mut c_void>()
    }
}

#[inline(always)]
unsafe fn rb_right(node: *const c_void) -> *mut c_void {
    unsafe {
        *(node as *const u8).add(RB_RIGHT_OFF).cast::<*mut c_void>()
    }
}

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

// Safety limit on rb_next/rb_prev walks.  A real rb_tree is depth
// O(log n).  Even unbalanced trees (since our rb_insert_color is a
// no-op) shouldn't exceed n nodes ≈ 1000s for fs use cases.  10K
// caps any pathological cycle without limiting practical traversal.
const RB_WALK_LIMIT: usize = 10_000;

/// Find the in-order successor of `node`.  Standard rb-tree walk:
/// if right child exists, descend right then walk all the way left;
/// otherwise walk up via parents until we find one whose left child
/// is the chain we came from.
#[unsafe(no_mangle)]
pub extern "C" fn rb_next(node: *const c_void) -> *mut c_void {
    if node.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let r = rb_right(node);
        if !r.is_null() {
            let mut cur = r;
            for _ in 0..RB_WALK_LIMIT {
                let l = rb_left(cur);
                if l.is_null() { return cur; }
                cur = l;
            }
            log::warn!("kabi: rb_next: descent loop limit hit");
            return core::ptr::null_mut();
        }
        // No right subtree — walk up.  Because the kABI no-op
        // `rb_insert_color` doesn't rebalance, ext4-built rb_trees
        // can be malformed in subtle ways that produce parent
        // cycles (especially when ext4 calls rb_erase via our
        // no-op stub — the tree doesn't shrink, leaving stale
        // parent links).  Detect a cycle if we ever revisit the
        // original `node` and return NULL.
        let mut cur: *const c_void = node;
        for _ in 0..RB_WALK_LIMIT {
            let parent = rb_parent(cur);
            if parent.is_null() || parent == (cur as *mut c_void)
                || parent == (node as *mut c_void) {
                return core::ptr::null_mut();
            }
            if rb_left(parent) == (cur as *mut c_void) {
                return parent;
            }
            cur = parent;
        }
        core::ptr::null_mut()
    }
}

/// In-order predecessor — mirror of `rb_next`.
#[unsafe(no_mangle)]
pub extern "C" fn rb_prev(node: *const c_void) -> *mut c_void {
    if node.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let l = rb_left(node);
        if !l.is_null() {
            let mut cur = l;
            for _ in 0..RB_WALK_LIMIT {
                let r = rb_right(cur);
                if r.is_null() { return cur; }
                cur = r;
            }
            return core::ptr::null_mut();
        }
        let mut cur: *const c_void = node;
        for _ in 0..RB_WALK_LIMIT {
            let parent = rb_parent(cur);
            if parent.is_null() {
                return core::ptr::null_mut();
            }
            if rb_right(parent) == (cur as *mut c_void) {
                return parent;
            }
            cur = parent;
        }
        core::ptr::null_mut()
    }
}

ksym!(rb_erase);
ksym!(rb_insert_color);
ksym!(rb_first_postorder);
ksym!(rb_next_postorder);
ksym!(rb_next);
ksym!(rb_prev);
