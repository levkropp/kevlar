// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 early boot initialization.
use super::{cpu_local, gic, serial, smp, timer, CPU_ID};
use crate::address::{PAddr, VAddr};
use crate::bootinfo::BootInfo;
use crate::logger;
use crate::page_allocator;
use core::arch::asm;
use core::sync::atomic::Ordering;

unsafe extern "Rust" {
    fn boot_kernel(bootinfo: &BootInfo) -> !;
    fn ap_kernel_entry() -> !;
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

    // Initialize CPU-local area for BSP.
    cpu_local::init(VAddr::new(&__bsp_cpu_local as *const _ as usize));

    // Initialize GIC, serial, and timer.
    gic::init();
    serial::init();
    timer::init();

    // Wake Application Processors.
    smp::init(&boot_info.cpu_mpdirs);

    boot_kernel(&boot_info);
}

/// Entry point for Application Processors after MMU is enabled.
/// Called from `secondary_long_mode` in boot.S.
#[unsafe(no_mangle)]
unsafe extern "C" fn secondary_rust_entry() -> ! {
    let cpu_local_vaddr = VAddr::new(smp::AP_CPU_LOCAL.load(Ordering::Acquire) as usize);
    let ap_cpu_id = smp::AP_CPU_ID.load(Ordering::Acquire);

    // Initialize this CPU's cpu_local area and set TPIDR_EL1.
    cpu_local::init(cpu_local_vaddr);

    // Set per-CPU ID now that TPIDR_EL1 is established.
    CPU_ID.set(ap_cpu_id);

    // Enable this CPU's GIC interface.
    gic::init_ap();

    // Read MPIDR for logging.
    let mpidr: u64;
    asm!("mrs {}, mpidr_el1", out(reg) mpidr);
    info!("CPU (MPIDR {:#x}, cpu_id={}) online", mpidr & 0x00FF_FFFF, ap_cpu_id);

    // Signal the BSP that this AP is online.
    smp::AP_ONLINE_COUNT.fetch_add(1, Ordering::Release);

    // Hand off to the kernel for per-CPU process state initialization.
    ap_kernel_entry()
}
