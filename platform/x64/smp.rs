// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! x86_64 SMP (Symmetric Multi-Processing) boot.
//!
//! `init()` is called by the BSP after its own per-CPU setup is complete.
//! It discovers Application Processors via the ACPI MADT, allocates per-AP
//! kernel stacks and cpu_local areas, then wakes each AP with the Intel
//! INIT-SIPI-SIPI sequence.
//!
//! Each AP executes the trampoline at physical 0x8000, which transitions from
//! 16-bit real mode to 64-bit long mode and jumps to `long_mode` in boot.S.
//! `boot.S` reads the LAPIC ID and calls `ap_rust_entry` (in boot.rs).
//! `ap_rust_entry` performs per-CPU setup, signals `AP_ONLINE_COUNT`, then
//! enters the idle loop.

use super::acpi;
use super::cpu_local::CpuLocalHead;
use super::{apic, tsc};
use crate::page_allocator::{alloc_pages, AllocPageFlags};
use crate::arch::PAGE_SIZE;
use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering, fence};
use core::ptr;

/// Maximum CPUs supported — matches flight_recorder::MAX_CPUS.
pub const MAX_CPUS: usize = 8;

/// Pointers to each online CPU's CpuLocalHead, indexed by cpu_id.
///
/// Populated by the BSP (slot 0) in `bsp_early_init` and by each AP in
/// `ap_rust_entry`, after `cpu_local::init` has set GSBASE.  Readers
/// (e.g. the Vm::Drop grace-period scan in `paging::read_all_qsc_counters`)
/// use this table to read each CPU's `qsc_counter` without having to
/// execute on that CPU.
///
/// A slot is null until the corresponding CPU has finished its per-CPU
/// init; consumers must skip null slots.
pub static CPU_LOCAL_HEADS: [AtomicPtr<CpuLocalHead>; MAX_CPUS] = [
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
    AtomicPtr::new(ptr::null_mut()),
];

/// Register the currently-executing CPU's CpuLocalHead pointer into the
/// global table.  Must be called AFTER `cpu_local::init` has set GSBASE.
pub fn register_cpu_local_head(cpu_id: u32) {
    if (cpu_id as usize) >= MAX_CPUS {
        return;
    }
    let head = super::cpu_local::cpu_local_head() as *mut CpuLocalHead;
    CPU_LOCAL_HEADS[cpu_id as usize].store(head, Ordering::Release);
}

// ── AP stack and cpu_local sizing ─────────────────────────────────────

/// Kernel stack size per AP: 16 pages = 64 KiB (matches BSP boot stack).
pub const AP_STACK_PAGES: usize = 16;

/// Number of CPUs that have completed ap_rust_entry setup.
/// BSP is not counted here; add 1 to get total online CPUs.
pub static AP_ONLINE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Paddr ranges of the AP stacks allocated at boot.  Exposed to
/// `platform/page_allocator.rs::free_pages` so we can panic if anyone
/// tries to free a single page within a live AP-stack allocation — the
/// AP stack is never freed for the kernel's lifetime, so a free here
/// is a smoking-gun bug (task #25 hunting).  Indexed by cpu_id (BSP
/// is cpu 0 which never registers — it uses the static boot stack).
pub static AP_STACK_BASES: [core::sync::atomic::AtomicUsize; MAX_CPUS] = [
    const { core::sync::atomic::AtomicUsize::new(0) }; MAX_CPUS
];

/// Returns true if `paddr` is inside any AP's live kernel stack range.
pub fn is_ap_stack_paddr(paddr: crate::address::PAddr) -> bool {
    use core::sync::atomic::Ordering;
    let p = paddr.value();
    for slot in AP_STACK_BASES.iter() {
        let base = slot.load(Ordering::Relaxed);
        if base != 0 && p >= base && p < base + AP_STACK_PAGES * crate::arch::PAGE_SIZE {
            return true;
        }
    }
    false
}

/// The cpu_local area VAddr for the AP currently being started.
/// The BSP writes this before sending SIPI; the AP reads it on entry.
/// APs are started one at a time so a single cell suffices.
pub static AP_CPU_LOCAL: AtomicUsize = AtomicUsize::new(0);

/// The cpu index (0=BSP, 1..N=AP order) for the AP currently being started.
/// Written by BSP before SIPI; read by AP in ap_rust_entry.
pub static AP_CPU_ID: AtomicU32 = AtomicU32::new(0);

// ── Trampoline data symbols (in .trampoline section at VMA 0x8000) ───

unsafe extern "C" {
    static ap_tram_cr3:   u32;
    static ap_tram_stack: u64;
}

// ── Public API ────────────────────────────────────────────────────────

/// Returns the total number of online CPUs (BSP + all APs that have
/// completed `ap_rust_entry`).
pub fn num_online_cpus() -> u32 {
    AP_ONLINE_COUNT.load(Ordering::Relaxed) + 1
}

/// Wake all Application Processors found in the ACPI MADT.
/// Must be called after `page_allocator::init()` and `common_setup()`.
pub unsafe fn init() {
    let (lapic_ids, count) = acpi::find_lapic_ids();
    if count == 0 {
        info!("smp: no APs found, running single-CPU");
        return;
    }

    // BSP APIC ID (typically 0, but read from hardware to be sure).
    let bsp_id = apic::lapic_id();

    // CR3 value (physical address of PML4) — read from the current CPU.
    let cr3 = x86::controlregs::cr3() as u32;

    // cpu_local size from the linker script symbol.
    let cpu_local_size = cpu_local_size();
    let cpu_local_pages = (cpu_local_size + PAGE_SIZE - 1) / PAGE_SIZE;

    let mut started = 0u32;
    let mut next_cpu_id: u32 = 1; // BSP is 0; APs get 1, 2, ...

    for i in 0..count {
        let apic_id = lapic_ids[i];
        if apic_id == bsp_id {
            continue; // Skip the BSP.
        }

        // Allocate AP kernel stack (contiguous pages, zeroed).
        let stack_paddr = match alloc_pages(AP_STACK_PAGES, AllocPageFlags::empty()) {
            Ok(p) => p,
            Err(_) => {
                warn!("smp: failed to allocate stack for APIC ID {}", apic_id);
                continue;
            }
        };
        // Register the AP stack's paddr so is_ap_stack_paddr() detects
        // anyone trying to free into this region.
        if (next_cpu_id as usize) < MAX_CPUS {
            AP_STACK_BASES[next_cpu_id as usize].store(
                stack_paddr.value(),
                Ordering::Release,
            );
        }
        // Stack pointer is at the top of the allocated region.
        let stack_top = stack_paddr.add(AP_STACK_PAGES * PAGE_SIZE).as_vaddr().value() as u64;

        // Allocate AP cpu_local area (zeroed).
        let cpu_local_paddr = match alloc_pages(cpu_local_pages.max(1), AllocPageFlags::empty()) {
            Ok(p) => p,
            Err(_) => {
                warn!("smp: failed to allocate cpu_local for APIC ID {}", apic_id);
                continue;
            }
        };
        let cpu_local_vaddr = cpu_local_paddr.as_vaddr();

        // Publish the cpu_local area and cpu index to the AP (read by ap_rust_entry).
        AP_CPU_ID.store(next_cpu_id, Ordering::Release);
        AP_CPU_LOCAL.store(cpu_local_vaddr.value(), Ordering::Release);

        // Write CR3 and stack into the trampoline data page.
        // ap_tram_cr3 and ap_tram_stack are at VMA 0x8060ish (identity-mapped).
        // Use addr_of! to avoid creating a &T reference to a mutable static.
        let cr3_ptr   = ptr::addr_of!(ap_tram_cr3)   as *mut u32;
        let stack_ptr = ptr::addr_of!(ap_tram_stack)  as *mut u64;
        ptr::write_volatile(cr3_ptr, cr3);
        ptr::write_volatile(stack_ptr, stack_top);

        // Ensure the trampoline writes are visible before the SIPI.
        fence(Ordering::Release);

        let prev_count = AP_ONLINE_COUNT.load(Ordering::Relaxed);

        // INIT-SIPI-SIPI protocol (Intel SDM, Volume 3, §8.4.4).
        apic::send_init_ipi(apic_id);
        udelay(10_000); // 10 ms

        apic::send_sipi(apic_id, 0x08); // vector 0x08 → physical 0x8000
        udelay(200);                     // 200 µs

        apic::send_sipi(apic_id, 0x08); // second SIPI (some CPUs need two)

        // Wait up to 200 ms for the AP to signal that it is online.
        let deadline = tsc::nanoseconds_since_boot() + 200_000_000;
        loop {
            if AP_ONLINE_COUNT.load(Ordering::Acquire) > prev_count {
                started += 1;
                next_cpu_id += 1;
                break;
            }
            if tsc::nanoseconds_since_boot() >= deadline {
                warn!("smp: APIC ID {} did not come online", apic_id);
                break;
            }
            core::hint::spin_loop();
        }
    }

    info!("smp: {} AP(s) online, total {} CPU(s)", started, started + 1);
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Spin for approximately `us` microseconds using the TSC.
/// Requires TSC calibration to have completed (it always has by the time
/// smp::init() is called).
fn udelay(us: u64) {
    let start = tsc::nanoseconds_since_boot();
    let end   = start + us * 1_000;
    while tsc::nanoseconds_since_boot() < end {
        core::hint::spin_loop();
    }
}

/// Size of the cpu_local template (from linker script symbol `__cpu_local_size`).
fn cpu_local_size() -> usize {
    unsafe extern "C" {
        static __cpu_local_size: u8;
    }
    // The address of __cpu_local_size IS the size (linker script defines it as
    // `__cpu_local_size = __cpu_local_end - __cpu_local`).
    unsafe { &__cpu_local_size as *const u8 as usize }
}
