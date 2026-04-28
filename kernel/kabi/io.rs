// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux MMIO + ioremap shims for K5.
//!
//! `readl(addr)` / `writel(val, addr)` etc. are volatile loads/stores.
//! `ioremap(phys, size)` returns the kernel direct-map VA for the
//! given physical address — sufficient for any phys below the
//! direct-map ceiling (~4 GB on QEMU virt arm64).  Real ioremap
//! with page-table allocation in the vmalloc area defers to K6.

use core::ffi::c_void;

use kevlar_platform::address::PAddr;

use crate::ksym;

// ── MMIO accessors ──────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn readb(addr: *const c_void) -> u8 {
    if addr.is_null() {
        return 0;
    }
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

#[unsafe(no_mangle)]
pub extern "C" fn readw(addr: *const c_void) -> u16 {
    if addr.is_null() {
        return 0;
    }
    unsafe { core::ptr::read_volatile(addr as *const u16) }
}

#[unsafe(no_mangle)]
pub extern "C" fn readl(addr: *const c_void) -> u32 {
    if addr.is_null() {
        return 0;
    }
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[unsafe(no_mangle)]
pub extern "C" fn readq(addr: *const c_void) -> u64 {
    if addr.is_null() {
        return 0;
    }
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

#[unsafe(no_mangle)]
pub extern "C" fn writeb(val: u8, addr: *mut c_void) {
    if addr.is_null() {
        return;
    }
    unsafe { core::ptr::write_volatile(addr as *mut u8, val) }
}

#[unsafe(no_mangle)]
pub extern "C" fn writew(val: u16, addr: *mut c_void) {
    if addr.is_null() {
        return;
    }
    unsafe { core::ptr::write_volatile(addr as *mut u16, val) }
}

#[unsafe(no_mangle)]
pub extern "C" fn writel(val: u32, addr: *mut c_void) {
    if addr.is_null() {
        return;
    }
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

#[unsafe(no_mangle)]
pub extern "C" fn writeq(val: u64, addr: *mut c_void) {
    if addr.is_null() {
        return;
    }
    unsafe { core::ptr::write_volatile(addr as *mut u64, val) }
}

// ── ioremap ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn ioremap(phys: u64, _size: usize) -> *mut c_void {
    let pa = PAddr::new(phys as usize);
    pa.as_vaddr().value() as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn ioremap_wc(phys: u64, size: usize) -> *mut c_void {
    ioremap(phys, size)
}

#[unsafe(no_mangle)]
pub extern "C" fn ioremap_nocache(phys: u64, size: usize) -> *mut c_void {
    ioremap(phys, size)
}

#[unsafe(no_mangle)]
pub extern "C" fn ioremap_cache(phys: u64, size: usize) -> *mut c_void {
    ioremap(phys, size)
}

#[unsafe(no_mangle)]
pub extern "C" fn iounmap(_addr: *mut c_void) {
    // K5: no-op; the kernel direct map is permanent.
}

ksym!(readb);
ksym!(readw);
ksym!(readl);
ksym!(readq);
ksym!(writeb);
ksym!(writew);
ksym!(writel);
ksym!(writeq);
ksym!(ioremap);
ksym!(ioremap_wc);
ksym!(ioremap_nocache);
ksym!(ioremap_cache);
ksym!(iounmap);

/// Legacy x86 port-I/O helpers.  cirrus-qemu / bochs inherit
/// references from i386 vga code; on aarch64 there is no port
/// I/O, so all writes are no-ops and reads return 0.
#[unsafe(no_mangle)]
pub extern "C" fn logic_outb(_value: u8, _port: u16) {}

#[unsafe(no_mangle)]
pub extern "C" fn logic_outw(_value: u16, _port: u16) {}

#[unsafe(no_mangle)]
pub extern "C" fn logic_inb(_port: u16) -> u8 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn logic_inw(_port: u16) -> u16 {
    0
}

ksym!(logic_outb);
ksym!(logic_outw);
ksym!(logic_inb);
ksym!(logic_inw);
