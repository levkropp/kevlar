// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! `kmalloc` / `vmalloc` family — Linux's memory-allocator surface.
//!
//! K2 ignores `gfp_t` flag bits.  Kevlar's heap allocator
//! (`platform/global_allocator.rs`) is already IRQ-safe via an
//! interrupt-disabling spinlock, so the GFP_KERNEL vs GFP_ATOMIC
//! distinction collapses.  `__GFP_ZERO` is honored implicitly via
//! `kzalloc`.
//!
//! `kfree` doesn't take a size argument, so each `kmalloc` allocation
//! carries a 16-byte header `[size: u64, _pad: u64]` in front of the
//! returned pointer.  16 bytes keeps the user-visible alignment at
//! 16 (matches `__BIGGEST_ALIGNMENT__` on aarch64 — what GCC assumes
//! for `kmalloc`-returned buffers).
//!
//! `vmalloc` allocates page-multiples via the page allocator and
//! tracks the size in a side table indexed by VA.  `kvmalloc`
//! dispatches by size threshold.

use alloc::alloc::{alloc, alloc_zeroed, dealloc, Layout};
use alloc::vec::Vec;
use core::ffi::c_void;

use kevlar_platform::address::{PAddr, VAddr};
use kevlar_platform::page_allocator::{alloc_pages, free_pages, AllocPageFlags};
use kevlar_platform::spinlock::SpinLock;

use crate::ksym;

const HEADER_SIZE: usize = 16;
const HEADER_ALIGN: usize = 16;
const PAGE_SIZE: usize = 4096;
const KVMALLOC_THRESHOLD: usize = 8 * 1024;

#[inline]
fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

fn kmalloc_layout(user_size: usize) -> Option<Layout> {
    let total = user_size.checked_add(HEADER_SIZE)?;
    Layout::from_size_align(total, HEADER_ALIGN).ok()
}

#[allow(unsafe_code)]
fn kmalloc_internal(size: usize, zero: bool) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let layout = match kmalloc_layout(size) {
        Some(l) => l,
        None => return core::ptr::null_mut(),
    };
    let raw = unsafe {
        if zero {
            alloc_zeroed(layout)
        } else {
            alloc(layout)
        }
    };
    if raw.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        core::ptr::write(raw as *mut u64, size as u64);
    }
    unsafe { raw.add(HEADER_SIZE) }
}

#[allow(unsafe_code)]
fn kfree_internal(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let raw = unsafe { ptr.sub(HEADER_SIZE) };
    let size = unsafe { core::ptr::read(raw as *const u64) } as usize;
    let layout = match kmalloc_layout(size) {
        Some(l) => l,
        None => return,
    };
    unsafe { dealloc(raw, layout) };
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn kmalloc(size: usize, _gfp: u32) -> *mut c_void {
    kmalloc_internal(size, false) as *mut c_void
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn kzalloc(size: usize, _gfp: u32) -> *mut c_void {
    kmalloc_internal(size, true) as *mut c_void
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn kcalloc(n: usize, size: usize, _gfp: u32) -> *mut c_void {
    let total = match n.checked_mul(size) {
        Some(t) => t,
        None => return core::ptr::null_mut(),
    };
    kmalloc_internal(total, true) as *mut c_void
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn krealloc(
    ptr: *mut c_void,
    new_size: usize,
    gfp: u32,
) -> *mut c_void {
    if ptr.is_null() {
        return kmalloc(new_size, gfp);
    }
    if new_size == 0 {
        kfree_internal(ptr as *mut u8);
        return core::ptr::null_mut();
    }
    let old_raw = unsafe { (ptr as *mut u8).sub(HEADER_SIZE) };
    let old_size = unsafe { core::ptr::read(old_raw as *const u64) } as usize;
    let new_ptr = kmalloc_internal(new_size, false);
    if new_ptr.is_null() {
        return core::ptr::null_mut();
    }
    let copy_len = old_size.min(new_size);
    unsafe {
        core::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr, copy_len);
    }
    kfree_internal(ptr as *mut u8);
    new_ptr as *mut c_void
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn kfree(ptr: *mut c_void) {
    kfree_internal(ptr as *mut u8);
}

ksym!(kmalloc);
ksym!(kzalloc);
ksym!(kcalloc);
ksym!(krealloc);
ksym!(kfree);

// ── Linux 7.0 kmalloc renames ─────────────────────────────────
//
// The MM subsystem renamed several allocator entry points to
// `_noprof` variants in 7.0 (memory-profiling support, gated
// by CONFIG_MEM_ALLOC_PROFILING).  Modules built against 7.0
// headers reference these names; we alias to our existing
// kmalloc/kzalloc.

#[unsafe(no_mangle)]
pub extern "C" fn __kmalloc_noprof(size: usize, gfp: u32) -> *mut core::ffi::c_void {
    kmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn __kmalloc_cache_noprof(
    _cache: *const core::ffi::c_void,
    gfp: u32,
    size: usize,
) -> *mut core::ffi::c_void {
    kmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn kvmalloc_node_noprof(
    size: usize,
    gfp: u32,
    _node: i32,
) -> *mut core::ffi::c_void {
    kvmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn kmemdup_noprof(
    src: *const core::ffi::c_void,
    size: usize,
    gfp: u32,
) -> *mut core::ffi::c_void {
    let dst = kmalloc(size, gfp);
    if !dst.is_null() && !src.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(
                src as *const u8,
                dst as *mut u8,
                size,
            );
        }
    }
    dst
}

#[unsafe(no_mangle)]
pub extern "C" fn devm_kmalloc(
    _dev: *mut core::ffi::c_void,
    size: usize,
    gfp: u32,
) -> *mut core::ffi::c_void {
    // K12: ignore the device-managed lifetime; just kmalloc.
    // Real Linux frees on device removal; ours leaks.
    kmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn devm_kmemdup(
    _dev: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    size: usize,
    gfp: u32,
) -> *mut core::ffi::c_void {
    kmemdup_noprof(src, size, gfp)
}

ksym!(__kmalloc_noprof);
ksym!(__kmalloc_cache_noprof);
ksym!(kvmalloc_node_noprof);
ksym!(kmemdup_noprof);
ksym!(devm_kmalloc);
ksym!(devm_kmemdup);

#[unsafe(no_mangle)]
pub extern "C" fn __kvmalloc_node_noprof(
    size: usize,
    gfp: u32,
    _node: i32,
) -> *mut core::ffi::c_void {
    kvmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn kvrealloc_node_align_noprof(
    p: *mut core::ffi::c_void,
    size: usize,
    _align: usize,
    gfp: u32,
    _node: i32,
) -> *mut core::ffi::c_void {
    // krealloc handles the heap-only path; drm_exec doesn't fire
    // this at load.  K15+ when a vmalloc-backed pointer is ever
    // realloc'd, we'll route via kvfree/kvmalloc.
    krealloc(p, size, gfp)
}

ksym!(__kvmalloc_node_noprof);
ksym!(kvrealloc_node_align_noprof);

// `kmalloc_caches` is a static array the kmalloc inlining
// machinery indexes into.  Modules read it to pick the right
// cache for a given size; our shim ignores the cache pointer.
// Provide a 1-page zero-filled static so the address resolves.
#[unsafe(no_mangle)]
pub static kmalloc_caches: [u8; 4096] = [0; 4096];
crate::ksym_static!(kmalloc_caches);

#[unsafe(no_mangle)]
pub static random_kmalloc_seed: u64 = 0;
crate::ksym_static!(random_kmalloc_seed);

// ── vmalloc ──────────────────────────────────────────────────────

/// Side table mapping (vmalloc-returned VA) → (PAddr, num_pages).
/// vmalloc'd memory comes from the page allocator (multi-page,
/// physically contiguous in K2 — true vmap is K3+); the side table
/// lets `vfree` recover the deallocation arguments without a header.
static VMALLOC_TABLE: SpinLock<Vec<VmallocEntry>> = SpinLock::new(Vec::new());

struct VmallocEntry {
    va: usize,
    paddr: PAddr,
    num_pages: usize,
}

#[allow(unsafe_code)]
fn vmalloc_internal(size: usize, zero: bool) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let num_pages = align_up(size, PAGE_SIZE) / PAGE_SIZE;
    let paddr = match alloc_pages(num_pages, AllocPageFlags::KERNEL) {
        Ok(p) => p,
        Err(_) => return core::ptr::null_mut(),
    };
    let va = paddr.as_vaddr().value();
    if zero {
        unsafe {
            core::ptr::write_bytes(va as *mut u8, 0, num_pages * PAGE_SIZE);
        }
    }
    VMALLOC_TABLE.lock().push(VmallocEntry {
        va,
        paddr,
        num_pages,
    });
    va as *mut u8
}

#[allow(unsafe_code)]
fn vfree_internal(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let va = ptr as usize;
    let mut tbl = VMALLOC_TABLE.lock();
    let pos = match tbl.iter().position(|e| e.va == va) {
        Some(p) => p,
        None => {
            log::warn!("kabi: vfree of untracked pointer {:#x}", va);
            return;
        }
    };
    let entry = tbl.remove(pos);
    drop(tbl);
    free_pages(entry.paddr, entry.num_pages);
}

#[unsafe(no_mangle)]
pub extern "C" fn vmalloc(size: usize) -> *mut c_void {
    vmalloc_internal(size, false) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn vzalloc(size: usize) -> *mut c_void {
    vmalloc_internal(size, true) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn vfree(ptr: *mut c_void) {
    vfree_internal(ptr as *mut u8);
}

ksym!(vmalloc);
ksym!(vzalloc);
ksym!(vfree);

// ── kvmalloc / kvfree ─────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn kvmalloc(size: usize, gfp: u32) -> *mut c_void {
    if size >= KVMALLOC_THRESHOLD {
        vmalloc(size)
    } else {
        kmalloc(size, gfp)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kvzalloc(size: usize, gfp: u32) -> *mut c_void {
    if size >= KVMALLOC_THRESHOLD {
        vzalloc(size)
    } else {
        kzalloc(size, gfp)
    }
}

/// `kvfree` must distinguish heap-backed vs page-backed pointers.
/// We probe the vmalloc side table; absence ⇒ heap-backed.
#[unsafe(no_mangle)]
pub extern "C" fn kvfree(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    let va = ptr as usize;
    let in_vmalloc = VMALLOC_TABLE.lock().iter().any(|e| e.va == va);
    if in_vmalloc {
        vfree_internal(ptr as *mut u8);
    } else {
        kfree_internal(ptr as *mut u8);
    }
}

ksym!(kvmalloc);
ksym!(kvzalloc);
ksym!(kvfree);

// Suppress unused import warnings if VAddr ends up not needed in
// some configuration.
#[allow(dead_code)]
fn _unused_imports(_: VAddr) {}
