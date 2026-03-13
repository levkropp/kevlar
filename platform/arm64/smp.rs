// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 SMP boot via PSCI CPU_ON.
//!
//! For each AP found in the DTB, the BSP:
//!   1. Allocates a kernel stack and cpu_local area.
//!   2. Publishes the pointers and cpu_id in atomic statics read by
//!      `secondary_rust_entry` in boot.rs.
//!   3. Calls PSCI CPU_ON (HVC #0) with `secondary_entry` as the entry point.
//!   4. Waits up to 1 s for the AP to increment `AP_ONLINE_COUNT`.
//!
//! APs are started one at a time so the shared atomic statics are safe.
use crate::arch::PAGE_SIZE;
use crate::page_allocator::{alloc_pages, AllocPageFlags};
use core::arch::asm;
use core::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};

// ── Handshake atomics (read by secondary_rust_entry) ──────────────────

/// Number of APs that have completed `secondary_rust_entry`.
/// BSP is not counted; add 1 for total online CPUs.
#[unsafe(no_mangle)]
pub static AP_ONLINE_COUNT: AtomicU32 = AtomicU32::new(0);

/// VAddr of cpu_local area for the AP currently being started.
#[unsafe(no_mangle)]
pub static AP_CPU_LOCAL: AtomicU64 = AtomicU64::new(0);

/// CPU index for the AP currently being started (BSP = 0, APs = 1..N).
#[unsafe(no_mangle)]
pub static AP_CPU_ID: AtomicU32 = AtomicU32::new(0);

/// Kernel stack top (VAddr) for the AP currently being started.
/// Read directly from assembly in `secondary_long_mode` before calling Rust.
#[unsafe(no_mangle)]
pub static AP_ENTRY_STACK_TOP: AtomicU64 = AtomicU64::new(0);

// ── Sizing ────────────────────────────────────────────────────────────

const AP_STACK_PAGES: usize = 16; // 64 KiB kernel stack per AP

// ── PSCI constants ────────────────────────────────────────────────────

/// PSCI SMC64 CPU_ON function ID.
const PSCI_CPU_ON: u64 = 0xC4000003;
/// PSCI return code: CPU is already on (treat as success).
const PSCI_ALREADY_ON: i64 = -4;
/// PSCI return code: invalid parameters (e.g. non-existent MPIDR).
const PSCI_INVALID_PARAMS: i64 = -2;

// ── Public API ────────────────────────────────────────────────────────

/// Returns total online CPUs (BSP + APs).
pub fn num_online_cpus() -> u32 {
    AP_ONLINE_COUNT.load(Ordering::Relaxed) + 1
}

/// Wake all Application Processors.
///
/// If `cpu_mpdirs` has more than one entry (parsed from a DTB), those MPIDRs
/// are used directly.  Otherwise (QEMU bare-metal ELF: no DTB in guest
/// memory), we probe sequential MPIDRs 1, 2, … via PSCI CPU_ON and stop
/// when PSCI returns INVALID_PARAMS (non-existent CPU).
///
/// Must be called after `page_allocator::init()`, `cpu_local::init()`,
/// `gic::init()`, and `timer::init()` have run on the BSP.
pub unsafe fn init(cpu_mpdirs: &[u64]) {
    // BSP MPIDR — bits [23:0] identify this CPU.
    let bsp_mpidr: u64;
    asm!("mrs {}, mpidr_el1", out(reg) bsp_mpidr);
    let bsp_mpidr = bsp_mpidr & 0x00FF_FFFF;

    // Physical address of secondary_entry (in .boot section where VMA = phys).
    unsafe extern "C" {
        fn secondary_entry();
    }
    let entry_paddr = secondary_entry as *const () as usize as u64;

    let cpu_local_pages = {
        unsafe extern "C" {
            static __cpu_local_size: u8;
        }
        let size = &__cpu_local_size as *const u8 as usize;
        (size + PAGE_SIZE - 1) / PAGE_SIZE
    };

    let mut started = 0u32;
    let mut next_cpu_id: u32 = 1;

    // Build iterator: either from DTB MPIDRs or PSCI probe (sequential 1..8).
    let use_probe = cpu_mpdirs.len() <= 1;
    let max_probe_mpidr: u64 = 8;

    let mut dtb_idx = 0usize;
    let mut probe_mpidr: u64 = 0;

    loop {
        // Get next candidate MPIDR.
        let mpidr = if use_probe {
            probe_mpidr += 1;
            if probe_mpidr >= max_probe_mpidr {
                break;
            }
            probe_mpidr
        } else {
            if dtb_idx >= cpu_mpdirs.len() {
                break;
            }
            let m = cpu_mpdirs[dtb_idx];
            dtb_idx += 1;
            m
        };

        if mpidr == bsp_mpidr {
            continue;
        }

        // Allocate AP kernel stack.
        let stack_paddr = match alloc_pages(AP_STACK_PAGES, AllocPageFlags::empty()) {
            Ok(p) => p,
            Err(_) => {
                warn!("smp: failed to alloc stack for MPIDR {:#x}", mpidr);
                break;
            }
        };
        let stack_top_vaddr = stack_paddr.add(AP_STACK_PAGES * PAGE_SIZE).as_vaddr().value();

        // Allocate AP cpu_local area.
        let cpu_local_paddr = match alloc_pages(cpu_local_pages.max(1), AllocPageFlags::empty()) {
            Ok(p) => p,
            Err(_) => {
                warn!("smp: failed to alloc cpu_local for MPIDR {:#x}", mpidr);
                break;
            }
        };
        let cpu_local_vaddr = cpu_local_paddr.as_vaddr().value();

        // Publish parameters — AP reads these in secondary_long_mode / secondary_rust_entry.
        AP_CPU_ID.store(next_cpu_id, Ordering::Relaxed);
        AP_CPU_LOCAL.store(cpu_local_vaddr as u64, Ordering::Relaxed);
        AP_ENTRY_STACK_TOP.store(stack_top_vaddr as u64, Ordering::Relaxed);
        fence(Ordering::Release);

        let prev_count = AP_ONLINE_COUNT.load(Ordering::Relaxed);

        // PSCI CPU_ON via HVC: x0=func_id, x1=target_mpidr, x2=entry, x3=ctx.
        let result: i64;
        asm!(
            "hvc #0",
            inlateout("x0") PSCI_CPU_ON => result,
            in("x1") mpidr,
            in("x2") entry_paddr,
            in("x3") 0u64,
            options(nostack),
        );

        if use_probe && result == PSCI_INVALID_PARAMS {
            // Non-existent CPU: stop probing (note: stack/cpu_local pages
            // allocated above are orphaned but harmless).
            break;
        }

        if result != 0 && result != PSCI_ALREADY_ON {
            warn!("smp: PSCI CPU_ON failed for MPIDR {:#x}: {}", mpidr, result);
            if use_probe {
                break; // unexpected error in probe mode: stop
            }
            continue;
        }

        // Wait up to 1 second (in ~1 ms increments) for the AP to come online.
        let mut waited = 0u32;
        loop {
            if AP_ONLINE_COUNT.load(Ordering::Acquire) > prev_count {
                started += 1;
                next_cpu_id += 1;
                break;
            }
            arm_delay_ms(1);
            waited += 1;
            if waited >= 1000 {
                warn!("smp: MPIDR {:#x} did not come online", mpidr);
                break;
            }
        }
    }

    if started == 0 {
        info!("smp: no APs found, running single-CPU");
    } else {
        info!("smp: {} AP(s) online, total {} CPU(s)", started, started + 1);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Busy-wait approximately `ms` milliseconds using the ARM generic counter.
fn arm_delay_ms(ms: u64) {
    let freq: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) freq) };
    let ticks = freq / 1000 * ms;
    let start: u64;
    unsafe { asm!("mrs {}, cntpct_el0", out(reg) start) };
    loop {
        let now: u64;
        unsafe { asm!("mrs {}, cntpct_el0", out(reg) now) };
        if now.wrapping_sub(start) >= ticks {
            break;
        }
        core::hint::spin_loop();
    }
}
