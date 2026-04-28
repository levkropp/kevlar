// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux DMA primitives for K5.
//!
//! `dma_alloc_coherent` returns a (kernel-VA, dma-PA) pair; on
//! arm64 QEMU virt the system is cache-coherent so the "DMA
//! address" is just the physical address.  `dma_map_single` /
//! `dma_unmap_single` collapse to no-ops.

use core::ffi::c_void;

use kevlar_platform::address::{PAddr, VAddr};
use kevlar_platform::page_allocator::{alloc_pages, AllocPageFlags};

use crate::kabi::device::DeviceShim;
use crate::ksym;

const PAGE_SIZE: usize = 4096;

#[inline]
fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_alloc_coherent(
    _dev: *mut DeviceShim,
    size: usize,
    dma_handle_out: *mut u64,
    _gfp: u32,
) -> *mut c_void {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let num_pages = align_up(size, PAGE_SIZE) / PAGE_SIZE;
    let pa = match alloc_pages(num_pages, AllocPageFlags::KERNEL) {
        Ok(p) => p,
        Err(_) => return core::ptr::null_mut(),
    };
    let va = pa.as_vaddr().value();
    unsafe {
        core::ptr::write_bytes(va as *mut u8, 0, num_pages * PAGE_SIZE);
    }
    if !dma_handle_out.is_null() {
        unsafe {
            *dma_handle_out = pa.value() as u64;
        }
    }
    va as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_free_coherent(
    _dev: *mut DeviceShim,
    _size: usize,
    _vaddr: *mut c_void,
    _dma_handle: u64,
) {
    // K5: leak the buffer.  Modules don't unload, and free_pages
    // tracking would need a side-table keyed by the returned VA.
    // Same simplification as the rest of K2-K4 unregistration.
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_map_single(
    _dev: *mut DeviceShim,
    ptr: *mut c_void,
    _size: usize,
    _dir: i32,
) -> u64 {
    if ptr.is_null() {
        return 0;
    }
    // arm64 QEMU virt is cache-coherent: just return the PA.
    let va = VAddr::new(ptr as usize);
    va.as_paddr().value() as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_unmap_single(
    _dev: *mut DeviceShim,
    _addr: u64,
    _size: usize,
    _dir: i32,
) {
    // No-op on cache-coherent arm64.
}

#[unsafe(no_mangle)]
pub extern "C" fn virt_to_phys(va: *mut c_void) -> u64 {
    if va.is_null() {
        return 0;
    }
    let v = VAddr::new(va as usize);
    v.as_paddr().value() as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn phys_to_virt(pa: u64) -> *mut c_void {
    let p = PAddr::new(pa as usize);
    p.as_vaddr().value() as *mut c_void
}

ksym!(dma_alloc_coherent);
ksym!(dma_free_coherent);
ksym!(dma_map_single);
ksym!(dma_unmap_single);
ksym!(virt_to_phys);
ksym!(phys_to_virt);

// ── K16 additions: Linux 7.0 _attrs / _pages variants + dma_buf
//    + sync helpers.  drm_dma_helper.ko references these but no
//    caller fires at load (no init_module).

#[unsafe(no_mangle)]
pub extern "C" fn dma_alloc_attrs(
    _dev: *mut c_void,
    _size: usize,
    _dma_handle: *mut u64,
    _flag: u32,
    _attrs: usize,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_free_attrs(
    _dev: *mut c_void,
    _size: usize,
    _vaddr: *mut c_void,
    _dma_handle: u64,
    _attrs: usize,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_alloc_pages(
    _dev: *mut c_void,
    _size: usize,
    _dma_handle: *mut u64,
    _dir: u32,
    _gfp: u32,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_free_pages(
    _dev: *mut c_void,
    _size: usize,
    _page: *mut c_void,
    _dma_handle: u64,
    _dir: u32,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_get_sgtable_attrs(
    _dev: *mut c_void,
    _sgt: *mut c_void,
    _cpu_addr: *mut c_void,
    _dma_addr: u64,
    _size: usize,
    _attrs: usize,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_mmap_attrs(
    _dev: *mut c_void,
    _vma: *mut c_void,
    _cpu_addr: *mut c_void,
    _dma_addr: u64,
    _size: usize,
    _attrs: usize,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_mmap_pages(
    _dev: *mut c_void,
    _vma: *mut c_void,
    _size: usize,
    _page: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_buf_vmap_unlocked(
    _dmabuf: *mut c_void,
    _map: *mut c_void,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn dma_buf_vunmap_unlocked(
    _dmabuf: *mut c_void,
    _map: *mut c_void,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn __dma_sync_single_for_device(
    _dev: *mut c_void,
    _addr: u64,
    _size: usize,
    _dir: u32,
) {
}

ksym!(dma_alloc_attrs);
ksym!(dma_free_attrs);
ksym!(dma_alloc_pages);
ksym!(dma_free_pages);
ksym!(dma_get_sgtable_attrs);
ksym!(dma_mmap_attrs);
ksym!(dma_mmap_pages);
ksym!(dma_buf_vmap_unlocked);
ksym!(dma_buf_vunmap_unlocked);
ksym!(__dma_sync_single_for_device);
