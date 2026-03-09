// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 early boot initialization.
use super::{cpu_local, gic, serial, timer};
use crate::address::{PAddr, VAddr};
use crate::bootinfo::BootInfo;
use crate::logger;
use crate::page_allocator;

unsafe extern "Rust" {
    fn boot_kernel(bootinfo: &BootInfo) -> !;
}

/// Called from boot.S after MMU is enabled and BSS is cleared.
#[unsafe(no_mangle)]
unsafe extern "C" fn bsp_early_init(dtb_paddr: u64) -> ! {
    unsafe extern "C" {
        static __bsp_cpu_local: u8;
    }

    // Initialize serial first to enable print macros.
    serial::early_init();
    logger::init();

    println!("Kevlar ARM64 booting...");
    println!("dtb_paddr = {:#x}", dtb_paddr);

    let boot_info = super::bootinfo::parse(PAddr::new(dtb_paddr as usize));
    println!("bootinfo OK, ram_areas={}", boot_info.ram_areas.len());
    if let Some(area) = boot_info.ram_areas.first() {
        println!("  ram0: {:#x}+{:#x}", area.base.value(), area.len);
    }
    page_allocator::init(&boot_info.ram_areas);
    println!("page_allocator OK");

    logger::set_log_filter(&boot_info.log_filter);

    // Initialize CPU-local area.
    cpu_local::init(VAddr::new(&__bsp_cpu_local as *const _ as usize));

    // Initialize GIC and timer.
    gic::init();
    serial::init();
    timer::init();

    boot_kernel(&boot_info);
}
