// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Red-black tree primitives for the kABI compat layer.
//!
//! ext4.ko (and other Linux fs/driver `.ko`s) call into a small set
//! of operations on `struct rb_node` whose ABI shape is fixed at
//! Linux's:
//!
//!   +0   unsigned long __rb_parent_color
//!        (low bit = color: 0 = RED, 1 = BLACK; remaining bits =
//!         parent pointer; nodes are 8-byte-aligned so this works)
//!   +8   struct rb_node *rb_right
//!   +16  struct rb_node *rb_left
//!
//! This file implements `rb_insert_color`, `rb_erase`, `rb_next`,
//! and `rb_prev` against that ABI shape.
//!
//! ## Provenance and licensing
//!
//! This is a clean-room implementation of the **classical red-black
//! tree** algorithm.  Algorithmic source: Cormen, Leiserson, Rivest,
//! Stein, _Introduction to Algorithms_, 3rd ed., Ch. 13 (the
//! canonical pedagogical reference — the algorithm is not
//! copyrightable and appears in dozens of permissively-licensed
//! implementations: Boost.Intrusive, jemalloc's `rb.h` (BSD-2-Clause),
//! libstdc++ `_Rb_tree`, `intrusive-collections` (Rust, MIT/Apache),
//! etc.).
//!
//! Structural style — direction-indexed helpers (`rotate(node, dir)`,
//! `child(node, dir)`, `elmdir/sibdir` case handling) — follows
//! FreeBSD's `sys/sys/tree.h` (BSD-2-Clause, Niels Provos 2002).
//! FreeBSD's tree itself uses a different (HST/rank-balanced)
//! algorithm with different bit semantics, so the algorithm here
//! is classical RB; the macro/helper organization is the borrowed
//! part.
//!
//! What this is **not**: a port or rewrite of Linux's
//! `lib/rbtree.c` (GPL-2.0).  Variable names, case numbering,
//! function decomposition, and code expression here are
//! independent of that source.
//!
//! What we read from Linux at all: only the `struct rb_node` field
//! offsets and the function signatures of `rb_insert_color` /
//! `rb_erase` / `rb_next` / `rb_prev` / `rb_first_postorder` /
//! `rb_next_postorder` — these are interface specifications (the
//! kABI), which under settled US law (cf. *Oracle v. Google*, 2021)
//! are not copyrightable for compatibility purposes.

use core::ffi::c_void;

use crate::ksym;

// ── struct rb_node field offsets (kABI; matches Linux 7.0) ──────

const PARENT_COLOR_OFF: usize = 0;
const RIGHT_OFF: usize = 8;
const LEFT_OFF: usize = 16;

// Color bit values.  Linux's lib/rbtree.c uses (RED=0, BLACK=1)
// and we follow because the encoding lives in struct rb_node which
// is shared with ext4.ko.  This isn't a creative choice — it's the
// ABI.
const RED: usize = 0;
const BLACK: usize = 1;

// Direction indices.  We index left/right by a 0/1 axis so that
// rotation and case handling can be written once with a `dir`
// parameter rather than mirrored.  This is the FreeBSD tree.h
// idiom (their `elmdir` / `sibdir` / `_RB_LR` macros).
const LEFT: usize = 0;
const RIGHT: usize = 1;

#[inline(always)]
fn other(dir: usize) -> usize { dir ^ 1 }

// ── primitive accessors ─────────────────────────────────────────

#[inline(always)]
unsafe fn pc_load(n: *const c_void) -> usize {
    unsafe { *(n as *const u8).add(PARENT_COLOR_OFF).cast::<usize>() }
}

#[inline(always)]
unsafe fn pc_store(n: *mut c_void, v: usize) {
    unsafe { *(n as *mut u8).add(PARENT_COLOR_OFF).cast::<usize>() = v; }
}

#[inline(always)]
unsafe fn parent_of(n: *const c_void) -> *mut c_void {
    if n.is_null() { return core::ptr::null_mut(); }
    (unsafe { pc_load(n) } & !1usize) as *mut c_void
}

#[inline(always)]
unsafe fn color_of(n: *const c_void) -> usize {
    unsafe { pc_load(n) & 1 }
}

#[inline(always)]
unsafe fn is_red(n: *const c_void) -> bool {
    !n.is_null() && unsafe { color_of(n) == RED }
}

#[inline(always)]
unsafe fn is_black(n: *const c_void) -> bool {
    n.is_null() || unsafe { color_of(n) == BLACK }
}

#[inline(always)]
unsafe fn set_pc(n: *mut c_void, p: *mut c_void, color: usize) {
    unsafe { pc_store(n, (p as usize) | color); }
}

#[inline(always)]
unsafe fn set_parent(n: *mut c_void, p: *mut c_void) {
    let c = unsafe { color_of(n) };
    unsafe { pc_store(n, (p as usize) | c); }
}

#[inline(always)]
unsafe fn set_color(n: *mut c_void, color: usize) {
    let p = unsafe { pc_load(n) & !1usize };
    unsafe { pc_store(n, p | color); }
}

#[inline(always)]
unsafe fn child(n: *const c_void, dir: usize) -> *mut c_void {
    let off = if dir == LEFT { LEFT_OFF } else { RIGHT_OFF };
    unsafe { *(n as *const u8).add(off).cast::<*mut c_void>() }
}

#[inline(always)]
unsafe fn set_child(n: *mut c_void, dir: usize, c: *mut c_void) {
    let off = if dir == LEFT { LEFT_OFF } else { RIGHT_OFF };
    unsafe { *(n as *mut u8).add(off).cast::<*mut c_void>() = c; }
}

#[inline(always)]
unsafe fn dir_of(parent: *const c_void, n: *const c_void) -> usize {
    if unsafe { child(parent, LEFT) } == (n as *mut c_void) { LEFT } else { RIGHT }
}

/// Replace `old` with `new` in `parent`'s child slot, or in
/// `*root_node` if `parent == NULL`.  `root` is a `*mut rb_root`
/// which has `rb_node` at offset 0.
#[inline(always)]
unsafe fn swap_child(
    root: *mut c_void, parent: *mut c_void,
    old: *mut c_void, new: *mut c_void,
) {
    unsafe {
        if parent.is_null() {
            *(root as *mut *mut c_void) = new;
        } else {
            let d = dir_of(parent, old);
            set_child(parent, d, new);
        }
    }
}

/// Single rotation around `pivot` in direction `dir`.  After:
/// `pivot`'s child in direction `dir` is what was its grandchild in
/// `(dir, dir^1)`; `pivot` becomes the (dir^1)-child of its old
/// (dir^1)-child.  Updates the link from `pivot`'s parent (or root)
/// to point at the new subtree top, and fixes parent pointers of
/// the moved subtree.  Returns the new subtree top.
unsafe fn rotate(root: *mut c_void, pivot: *mut c_void, dir: usize) -> *mut c_void {
    unsafe {
        let opp = other(dir);
        let new_top = child(pivot, opp);
        let inner = child(new_top, dir);

        // Move inner from new_top's `dir` slot to pivot's `opp` slot.
        set_child(pivot, opp, inner);
        if !inner.is_null() {
            set_parent(inner, pivot);
        }

        // Hoist new_top into pivot's place.
        let pivot_parent = parent_of(pivot);
        set_parent(new_top, pivot_parent);
        swap_child(root, pivot_parent, pivot, new_top);

        // Pivot becomes new_top's `dir` child.
        set_child(new_top, dir, pivot);
        set_parent(pivot, new_top);

        new_top
    }
}

// ── rb_insert_color ─────────────────────────────────────────────
//
// CLRS Ch. 13.3 "RB-Insert-Fixup".  The newly-linked node `z` is
// red; we walk up from `z` repairing red-red violations by either
// recoloring (when the uncle is red) or rotating (when the uncle is
// black).  Direction-indexed: rather than two mirrored branches,
// we read parent's direction within grandparent each iteration
// and use that to drive the sibling/rotation directions.

/// Real `rb_insert_color`.
#[unsafe(no_mangle)]
pub extern "C" fn rb_insert_color(node: *mut c_void, root: *mut c_void) {
    if node.is_null() || root.is_null() { return; }
    unsafe {
        let mut z = node;
        loop {
            let p = parent_of(z);
            if p.is_null() {
                // z became the root; force black.
                set_color(z, BLACK);
                return;
            }
            if is_black(p) {
                // No red-red violation; done.
                return;
            }
            // Parent is red, so grandparent must exist (root is
            // black — checked at the previous loop level — and a
            // red parent therefore has a non-NULL parent of its own).
            let gp = parent_of(p);
            if gp.is_null() {
                // Defensive: caller set up an inconsistent tree.
                set_color(p, BLACK);
                return;
            }

            // Direction of p within gp; uncle is the opposite child.
            let pdir = dir_of(gp, p);
            let udir = other(pdir);
            let u = child(gp, udir);

            if is_red(u) {
                // Case A: uncle red — push the violation up two
                // levels by recoloring.
                set_color(p, BLACK);
                set_color(u, BLACK);
                set_color(gp, RED);
                z = gp;
                continue;
            }

            // Case B: uncle black.  If z is on the inside (the
            // `udir` child of p), rotate at p to move z onto the
            // outside, then fall through to Case C.
            if dir_of(p, z) == udir {
                rotate(root, p, pdir);
                // After the inner rotation, the original z's role
                // is taken over by what was its other-direction
                // child; for the outer rotation we want the node
                // currently in p's old position.  Track that via
                // `p`.
                let new_p = z;
                z = p;
                let _ = new_p;
            }

            // Case C: z is on the outside.  Rotate at gp in the
            // uncle's direction; recolor old-parent black and
            // old-grandparent red.
            let p2 = parent_of(z);
            set_color(p2, BLACK);
            set_color(gp, RED);
            rotate(root, gp, udir);
            return;
        }
    }
}

// ── rb_erase ────────────────────────────────────────────────────
//
// CLRS Ch. 13.4 "RB-Delete" with sentinel-free pointer-walking
// (matching what the kABI ABI expects: NULLs for absent children,
// no T.nil sentinel).  Two phases:
//   1. Splice `z` out of the tree.  If z has two children, swap z
//      with its in-order successor first so the splice case is
//      always 0-or-1 child.
//   2. If a black node was removed, rebalance starting at the
//      removed node's parent.

/// Real `rb_erase`.
#[unsafe(no_mangle)]
pub extern "C" fn rb_erase(node: *mut c_void, root: *mut c_void) {
    if node.is_null() || root.is_null() { return; }
    unsafe {
        let z = node;

        // The "double-black" rebalance, if needed, starts at
        // `fix_parent` with phantom-black child `fix_child` (which
        // may be NULL).  We compute these from how we splice.
        let fix_parent: *mut c_void;
        let fix_child: *mut c_void;
        let removed_was_black: bool;

        if child(z, LEFT).is_null() {
            // Splice case: at most one child (right).
            removed_was_black = is_black(z);
            let r = child(z, RIGHT);
            let p = parent_of(z);
            swap_child(root, p, z, r);
            if !r.is_null() {
                set_parent(r, p);
            }
            fix_parent = p;
            fix_child = r;
        } else if child(z, RIGHT).is_null() {
            // One left child.
            removed_was_black = is_black(z);
            let l = child(z, LEFT);
            let p = parent_of(z);
            swap_child(root, p, z, l);
            if !l.is_null() {
                set_parent(l, p);
            }
            fix_parent = p;
            fix_child = l;
        } else {
            // Two children: find the in-order successor (leftmost
            // in z's right subtree) and splice IT, transplanting
            // z's links to it.  The "really removed" node from a
            // structural standpoint is the successor.
            let mut s = child(z, RIGHT);
            while !child(s, LEFT).is_null() {
                s = child(s, LEFT);
            }
            removed_was_black = is_black(s);
            let s_right = child(s, RIGHT);

            let s_parent: *mut c_void;
            if parent_of(s) == z {
                // Successor is z's direct right child.  After
                // splice, s's right child stays where it is; s
                // takes z's place; the rebalance starting point is
                // s itself (= s_parent for the phantom child
                // s_right).
                s_parent = s;
            } else {
                // Successor is deeper — its parent's left child
                // becomes s_right.
                let sp = parent_of(s);
                set_child(sp, LEFT, s_right);
                if !s_right.is_null() {
                    set_parent(s_right, sp);
                }
                // Hook z's right subtree under s.
                let zr = child(z, RIGHT);
                set_child(s, RIGHT, zr);
                set_parent(zr, s);
                s_parent = sp;
            }

            // Hoist s into z's place; copy z's color so the only
            // possible RB violation is from removing s's color.
            let zp = parent_of(z);
            swap_child(root, zp, z, s);
            let zl = child(z, LEFT);
            set_child(s, LEFT, zl);
            if !zl.is_null() {
                set_parent(zl, s);
            }
            set_pc(s, zp, color_of(z));

            fix_parent = s_parent;
            fix_child = s_right;
        }

        if removed_was_black {
            erase_fixup(root, fix_parent, fix_child);
        }
    }
}

/// CLRS Ch. 13.4 "RB-Delete-Fixup".  `child` is the (possibly NULL)
/// node currently sitting where the removed black node used to be;
/// `parent` is its parent.  We treat `child` as carrying an extra
/// phantom black token and walk up rotating/recoloring until the
/// violation is resolved.  Direction-indexed.
unsafe fn erase_fixup(root: *mut c_void, parent_in: *mut c_void, child_in: *mut c_void) {
    unsafe {
        let mut x = child_in;
        let mut xp = parent_in;
        while !xp.is_null() && is_black(x) {
            // Direction of x within parent.
            let xdir = if child(xp, LEFT) == x { LEFT } else { RIGHT };
            let sdir = other(xdir);
            let mut s = child(xp, sdir);

            // s cannot be NULL: parent had black-height ≥ 2 in
            // the absent (x) subtree, so the present (s) subtree
            // has black-height ≥ 2 → at least one node.  Defensive
            // check anyway:
            if s.is_null() { break; }

            // Step 1: if sibling is red, rotate at parent so the
            // new sibling is black.  Parent is now red; sibling's
            // old child (now sibling) is black.
            if is_red(s) {
                set_color(s, BLACK);
                set_color(xp, RED);
                rotate(root, xp, xdir);
                s = child(xp, sdir);
                if s.is_null() { break; }
            }

            let s_outer = child(s, sdir);
            let s_inner = child(s, xdir);

            if is_black(s_outer) && is_black(s_inner) {
                // Step 2: sibling and both nephews black.  Recolor
                // sibling red, push the phantom-black up to parent.
                set_color(s, RED);
                x = xp;
                xp = parent_of(x);
                continue;
            }

            // Step 3: outer nephew black, inner nephew red.  Rotate
            // at sibling so outer becomes red, then handle as Step 4.
            if is_black(s_outer) {
                if !s_inner.is_null() { set_color(s_inner, BLACK); }
                set_color(s, RED);
                rotate(root, s, sdir);
                s = child(xp, sdir);
                if s.is_null() { break; }
            }

            // Step 4: outer nephew red.  Rotate at parent; sibling
            // takes parent's color, parent and outer go black.  Done.
            set_color(s, color_of(xp));
            set_color(xp, BLACK);
            let s_outer2 = child(s, sdir);
            if !s_outer2.is_null() { set_color(s_outer2, BLACK); }
            rotate(root, xp, xdir);
            return;
        }
        if !x.is_null() {
            set_color(x, BLACK);
        }
    }
}

// ── postorder iteration: stubs (not currently exercised) ────────
//
// Used by rbtree_postorder_for_each_entry_safe, which appears in
// ext4's free_rb_tree_fname.  Stubbing makes that loop skip and
// leak whatever was in the tree — harmless for our test workloads
// (single-mount, short-lived dirs).  Implement when a workload
// actually needs the leak collected.

#[unsafe(no_mangle)]
pub extern "C" fn rb_first_postorder(_root: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn rb_next_postorder(_node: *const c_void) -> *mut c_void {
    core::ptr::null_mut()
}

// ── rb_next / rb_prev ───────────────────────────────────────────
//
// In-order successor and predecessor.  Standard tree walk: descend
// (right, then leftmost) for the successor, or walk up via parents
// looking for the first time we came from the left.  Mirror for
// predecessor.

#[unsafe(no_mangle)]
pub extern "C" fn rb_next(node: *const c_void) -> *mut c_void {
    if node.is_null() { return core::ptr::null_mut(); }
    unsafe {
        let r = child(node, RIGHT);
        if !r.is_null() {
            let mut cur = r;
            loop {
                let l = child(cur, LEFT);
                if l.is_null() { return cur; }
                cur = l;
            }
        }
        let mut cur: *const c_void = node;
        loop {
            let p = parent_of(cur);
            if p.is_null() { return core::ptr::null_mut(); }
            if child(p, LEFT) == (cur as *mut c_void) { return p; }
            cur = p;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rb_prev(node: *const c_void) -> *mut c_void {
    if node.is_null() { return core::ptr::null_mut(); }
    unsafe {
        let l = child(node, LEFT);
        if !l.is_null() {
            let mut cur = l;
            loop {
                let r = child(cur, RIGHT);
                if r.is_null() { return cur; }
                cur = r;
            }
        }
        let mut cur: *const c_void = node;
        loop {
            let p = parent_of(cur);
            if p.is_null() { return core::ptr::null_mut(); }
            if child(p, RIGHT) == (cur as *mut c_void) { return p; }
            cur = p;
        }
    }
}

ksym!(rb_erase);
ksym!(rb_insert_color);
ksym!(rb_first_postorder);
ksym!(rb_next_postorder);
ksym!(rb_next);
ksym!(rb_prev);
