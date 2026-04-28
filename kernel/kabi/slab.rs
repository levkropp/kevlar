// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux slab allocator (`kmem_cache_*`) shim.
//!
//! Linux's slab caches are size-class allocators tuned for repeated
//! same-size allocations.  K13 reduces them to plain `kmalloc`: each
//! cache is a 16-byte heap header that carries the object size; alloc
//! reads the size and calls `kmalloc`, free is just `kfree`.  No real
//! caching, no per-CPU magazines.  Sufficient for `drm_buddy.ko`
//! which only references these symbols (no callers in init).

use core::ffi::{c_char, c_void};

use crate::ksym;

/// Minimal cache descriptor — what `__kmem_cache_create_args` returns.
/// Modules treat the pointer as opaque.
#[repr(C)]
struct KmemCacheStub {
    object_size: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn __kmem_cache_create_args(
    _name: *const c_char,
    object_size: u32,
    _args: *const c_void,
    _flags: u32,
) -> *mut c_void {
    let cache = super::alloc::kmalloc(
        core::mem::size_of::<KmemCacheStub>(),
        0,
    ) as *mut KmemCacheStub;
    if cache.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        (*cache).object_size = object_size as usize;
    }
    cache as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn kmem_cache_alloc_noprof(
    cache: *mut c_void,
    gfp: u32,
) -> *mut c_void {
    if cache.is_null() {
        return core::ptr::null_mut();
    }
    let size = unsafe { (*(cache as *mut KmemCacheStub)).object_size };
    super::alloc::kmalloc(size, gfp)
}

#[unsafe(no_mangle)]
pub extern "C" fn kmem_cache_free(_cache: *mut c_void, ptr: *mut c_void) {
    super::alloc::kfree(ptr);
}

#[unsafe(no_mangle)]
pub extern "C" fn kmem_cache_destroy(cache: *mut c_void) {
    if !cache.is_null() {
        super::alloc::kfree(cache);
    }
}

ksym!(__kmem_cache_create_args);
ksym!(kmem_cache_alloc_noprof);
ksym!(kmem_cache_free);
ksym!(kmem_cache_destroy);
