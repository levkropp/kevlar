// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::{apic, bootinfo, cpu_local, gdt, idt, ioapic, pit, serial, smp, syscall, tss, vga};
use crate::address::{PAddr, VAddr};
use crate::bootinfo::BootInfo;
use crate::logger;
use crate::page_allocator;

use x86::{
    controlregs::{self, Cr4, Xcr0},
    cpuid::CpuId,
    io::outb,
};

fn check_cpuid_feature(name: &str, supported: bool) {
    if !supported {
        panic!("{} is not supprted on this machine", name);
    }
}

/// Per-AP CPU setup: a subset of `common_setup` that skips BSP-only
/// initializations (IOAPIC, PIT, TSC calibration, vDSO, serial).
unsafe fn ap_common_setup(cpu_local_area: VAddr) {
    // We re-check features mainly so that an incompatible AP panics loudly.
    let feats    = CpuId::new().get_feature_info().unwrap();
    let ex_feats = CpuId::new().get_extended_feature_info().unwrap();
    check_cpuid_feature("XSAVE",    feats.has_xsave());
    check_cpuid_feature("FSGSBASE", ex_feats.has_fsgsbase());

    let mut cr4 = controlregs::cr4();
    cr4 |= Cr4::CR4_ENABLE_FSGSBASE
        | Cr4::CR4_ENABLE_OS_XSAVE
        | Cr4::CR4_ENABLE_SSE
        | Cr4::CR4_UNMASKED_SSE;
    controlregs::cr4_write(cr4);

    let mut xcr0 = controlregs::xcr0();
    xcr0 |= Xcr0::XCR0_SSE_STATE | Xcr0::XCR0_AVX_STATE;
    controlregs::xcr0_write(xcr0);

    cpu_local::init(cpu_local_area);
    apic::init();
    // ioapic::init() — single IOAPIC, BSP-only.
    gdt::init();
    tss::init();
    idt::init();
    // tsc::calibrate() — global atomics already written by BSP, APs skip.
    // vdso::init()     — single vDSO page, BSP-only.
    // pit::init()      — single PIT, BSP-only.
    syscall::init();
}

/// Entry point for Application Processors.  Called from `setup_ap:` in
/// boot.S after the trampoline has set up long mode and the AP stack.
///
/// `lapic_id` is the AP's Local APIC ID read from the LAPIC ID register.
#[unsafe(no_mangle)]
unsafe extern "C" fn ap_rust_entry(lapic_id: u32) -> ! {
    let cpu_local_vaddr = VAddr::new(smp::AP_CPU_LOCAL.load(
        core::sync::atomic::Ordering::Acquire,
    ));

    ap_common_setup(cpu_local_vaddr);

    // All setup complete — announce and enter idle loop.
    info!("CPU (LAPIC {}) online", lapic_id);
    smp::AP_ONLINE_COUNT.fetch_add(1, core::sync::atomic::Ordering::Release);

    loop {
        super::idle::idle();
    }
}

/// Enables some CPU features.
unsafe fn common_setup(cpu_local_area: VAddr) {
    let feats = CpuId::new().get_feature_info().unwrap();
    let ex_feats = CpuId::new().get_extended_feature_info().unwrap();
    check_cpuid_feature("XSAVE", feats.has_xsave());
    check_cpuid_feature("FSGSBASE", ex_feats.has_fsgsbase());

    let mut cr4 = controlregs::cr4();
    cr4 |= Cr4::CR4_ENABLE_FSGSBASE
        | Cr4::CR4_ENABLE_OS_XSAVE
        | Cr4::CR4_ENABLE_SSE
        | Cr4::CR4_UNMASKED_SSE;
    controlregs::cr4_write(cr4);

    let mut xcr0 = controlregs::xcr0();
    xcr0 |= Xcr0::XCR0_SSE_STATE | Xcr0::XCR0_AVX_STATE;
    controlregs::xcr0_write(xcr0);

    cpu_local::init(cpu_local_area);
    apic::init();
    ioapic::init();
    gdt::init();
    tss::init();
    idt::init();
    // Calibrate TSC before PIT channel 0 is configured for timer IRQs.
    super::tsc::calibrate();
    super::vdso::init();
    pit::init();
    syscall::init();
}

/// Disables PIC. We use APIC instead.
unsafe fn init_pic() {
    outb(0xa1, 0xff);
    outb(0x21, 0xff);

    outb(0x20, 0x11);
    outb(0xa0, 0x11);
    outb(0x21, 0x20);
    outb(0xa1, 0x28);
    outb(0x21, 0x04);
    outb(0xa1, 0x02);
    outb(0x21, 0x01);
    outb(0xa1, 0x01);

    outb(0xa1, 0xff);
    outb(0x21, 0xff);
}

unsafe extern "Rust" {
    fn boot_kernel(bootinfo: &BootInfo) -> !;
}

/// Copy the AP trampoline from its ELF load address (after BSS) to physical
/// 0x8000, where SIPI vector 0x08 will execute it.  Must be called before
/// `smp::init()` sends any SIPIs.
unsafe fn copy_trampoline() {
    unsafe extern "C" {
        static __trampoline_start: u8;
        static __trampoline_end: u8;
        static __ap_trampoline_image: u8;
    }
    let size = (&raw const __trampoline_end as usize)
        - (&raw const __trampoline_start as usize);
    if size == 0 {
        return;
    }
    // __ap_trampoline_image is the physical LMA (after BSS); add VMA_OFFSET
    // to get the virtual address that is accessible at runtime.
    let src = ((&raw const __ap_trampoline_image as usize) | 0xffff800000000000) as *const u8;
    let dst = 0x8000usize as *mut u8;
    core::ptr::copy_nonoverlapping(src, dst, size);
}

/// Initializes the CPU. This function is called exactly once in the Bootstrap
/// Processor (BSP).
#[unsafe(no_mangle)]
unsafe extern "C" fn bsp_early_init(boot_magic: u32, boot_params: u64) -> ! {
    unsafe extern "C" {
        static __bsp_cpu_local: u8;
    }

    // Initialize the serial driver first to enable print macros.
    serial::early_init();
    vga::init();
    logger::init();

    // Copy the AP trampoline to physical 0x8000 BEFORE page_allocator::init()
    // claims that memory (the trampoline segment lands at the start of free RAM).
    copy_trampoline();

    let boot_info = bootinfo::parse(boot_magic, PAddr::new(boot_params as usize));
    page_allocator::init(&boot_info.ram_areas);

    logger::set_log_filter(&boot_info.log_filter);

    serial::init(boot_info.use_second_serialport);
    init_pic();
    common_setup(VAddr::new(&__bsp_cpu_local as *const _ as usize));
    smp::init();

    boot_kernel(&boot_info);
}
