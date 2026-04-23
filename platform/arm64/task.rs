// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Architecture-specific task (process) context for ARM64.
//!
//! This module was moved from kernel/arch/arm64/process.rs to consolidate
//! all unsafe code in the platform crate.
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::address::{AccessError, PAddr, UserVAddr, VAddr};
use crate::page_allocator::{alloc_pages_owned, AllocPageFlags, OwnedPages, PageAllocError};
use crate::arch::PAGE_SIZE;
use crate::arch::arm64_specific::cpu_local_head;
use crate::arch::PtRegs;
use crossbeam::atomic::AtomicCell;

/// Main kernel stack: 8 pages = 32 KiB.  Matches `platform/x64/task.rs::
/// KERNEL_STACK_SIZE`.  The previous value of 256 pages (1 MiB) multiplied
/// by three stacks per task made every fork pay for 3 MiB of buddy allocs
/// (167 µs fork_exit vs Linux's 16 µs).
pub const KERNEL_STACK_SIZE: usize = PAGE_SIZE * 8;

/// Auxiliary stacks (interrupt / syscall entry).  2 pages = 8 KiB is plenty
/// for a PtRegs push (~264 B) plus a handful of call frames, and it matches
/// the 2-page `stack_cache` size class so subsequent forks can reuse freed
/// stacks from the cache.
pub const AUX_STACK_PAGES: usize = 2;
pub const AUX_STACK_SIZE: usize = PAGE_SIZE * AUX_STACK_PAGES;

/// End of the user virtual address allocation region.
pub const USER_VALLOC_END: UserVAddr = unsafe { UserVAddr::new_unchecked(0x0000_0fff_0000_0000) };

/// Start of the user virtual address allocation region.
pub const USER_VALLOC_BASE: UserVAddr = unsafe { UserVAddr::new_unchecked(0x0000_000a_0000_0000) };

/// Top of the user stack (grows downward from USER_VALLOC_BASE).
pub const USER_STACK_TOP: UserVAddr = USER_VALLOC_BASE;

/// Architecture-specific process/task context for ARM64.
///
/// Contains the kernel stack pointer, TLS base, and allocated stacks.
pub struct ArchTask {
    sp: UnsafeCell<u64>,
    pub tpidr_el0: AtomicCell<u64>, // User TLS base (equivalent of fsbase)
    /// Set to `false` just before `do_switch_thread` saves this task's SP,
    /// then back to `true` by the assembly after the save.  `resume()` spins
    /// on this flag before enqueuing the task, preventing another CPU from
    /// loading a stale SP while the save is in flight.
    pub context_saved: AtomicBool,
    kernel_stack: OwnedPages,
    interrupt_stack: OwnedPages,
    syscall_stack: OwnedPages,
}

unsafe impl Sync for ArchTask {}

unsafe extern "C" {
    fn kthread_entry();
    fn userland_entry();
    fn forked_child_entry();
    fn do_switch_thread(prev_sp: *mut u64, next_sp: *const u64, ctx_saved: *mut u8);
}

unsafe fn push_stack(mut sp: *mut u64, value: u64) -> *mut u64 {
    unsafe {
        sp = sp.sub(1);
        sp.write(value);
    }
    sp
}

impl ArchTask {
    #[allow(unused)]
    pub fn new_kthread(ip: VAddr, stack_top: VAddr) -> ArchTask {
        let interrupt_stack = alloc_pages_owned(
            AUX_STACK_PAGES,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate interrupt stack");
        let syscall_stack = alloc_pages_owned(
            AUX_STACK_PAGES,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate syscall stack");
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");

        let sp = unsafe {
            let mut sp: *mut u64 = stack_top.as_mut_ptr();

            // Entry point for kthread_entry (popped and called via BLR).
            sp = push_stack(sp, ip.value() as u64);

            // Context matching do_switch_thread's save layout (low to high):
            //   NZCV, x29, x30, x27, x28, x25, x26, x23, x24, x21, x22, x19, x20
            // push_stack grows downward, so push in reverse (high to low).
            sp = push_stack(sp, 0); // x20
            sp = push_stack(sp, 0); // x19
            sp = push_stack(sp, 0); // x22
            sp = push_stack(sp, 0); // x21
            sp = push_stack(sp, 0); // x24
            sp = push_stack(sp, 0); // x23
            sp = push_stack(sp, 0); // x26
            sp = push_stack(sp, 0); // x25
            sp = push_stack(sp, 0); // x28
            sp = push_stack(sp, 0); // x27
            sp = push_stack(sp, kthread_entry as *const u8 as u64); // x30 (LR)
            sp = push_stack(sp, 0); // x29 (FP)
            sp = push_stack(sp, 0); // NZCV
            sp
        };

        ArchTask {
            sp: UnsafeCell::new(sp as u64),
            tpidr_el0: AtomicCell::new(0),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
    }

    pub fn new_user_thread(ip: UserVAddr, user_sp: UserVAddr) -> ArchTask {
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
        let interrupt_stack = alloc_pages_owned(
            AUX_STACK_PAGES,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate interrupt stack");
        let syscall_stack = alloc_pages_owned(
            AUX_STACK_PAGES,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate syscall stack");

        let sp = unsafe {
            let kernel_sp = kernel_stack.as_vaddr().add(KERNEL_STACK_SIZE);
            let mut sp: *mut u64 = kernel_sp.as_mut_ptr();

            // Push a PtRegs frame for userland_entry.
            // PtRegs: x0-x30 (31 regs), sp_el0, elr_el1, spsr_el1 = 34 u64s
            sp = sp.sub(34);
            let frame = sp as *mut u64;

            // Zero all x0-x30.
            for i in 0..31 {
                *frame.add(i) = 0;
            }
            *frame.add(31) = user_sp.value() as u64;  // sp_el0
            *frame.add(32) = ip.value() as u64;       // elr_el1 (entry point)
            *frame.add(33) = 0x0;                     // spsr_el1: EL0t, all interrupts unmasked

            // Context matching do_switch_thread's save layout (low to high):
            //   NZCV (8 bytes)
            //   x29, x30 (16 bytes)  -- ldp x29, x30
            //   x27, x28 (16 bytes)
            //   x25, x26 (16 bytes)
            //   x23, x24 (16 bytes)
            //   x21, x22 (16 bytes)
            //   x19, x20 (16 bytes)
            // push_stack grows downward, so push in reverse order (high to low).
            sp = push_stack(sp, 0); // x20
            sp = push_stack(sp, 0); // x19
            sp = push_stack(sp, 0); // x22
            sp = push_stack(sp, 0); // x21
            sp = push_stack(sp, 0); // x24
            sp = push_stack(sp, 0); // x23
            sp = push_stack(sp, 0); // x26
            sp = push_stack(sp, 0); // x25
            sp = push_stack(sp, 0); // x28
            sp = push_stack(sp, 0); // x27
            sp = push_stack(sp, userland_entry as *const u8 as u64); // x30 (LR)
            sp = push_stack(sp, 0); // x29 (FP)
            sp = push_stack(sp, 0); // NZCV
            sp
        };

        ArchTask {
            sp: UnsafeCell::new(sp as u64),
            tpidr_el0: AtomicCell::new(0),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
    }

    pub fn new_idle_thread() -> ArchTask {
        let interrupt_stack = alloc_pages_owned(
            AUX_STACK_PAGES,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate interrupt stack");
        let syscall_stack = alloc_pages_owned(
            AUX_STACK_PAGES,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate syscall stack");
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");

        ArchTask {
            sp: UnsafeCell::new(0),
            tpidr_el0: AtomicCell::new(0),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
    }

    pub fn fork(&self, frame: &PtRegs) -> Result<ArchTask, PageAllocError> {
        // Read the live hardware TPIDR_EL0.  The user process may have written
        // it directly via `msr tpidr_el0` (musl's __init_tp does this) without
        // going through any syscall, so ArchTask.tpidr_el0 may be stale (0).
        // We are in EL1 handling the fork SVC; the hardware register still
        // holds whatever EL0 last wrote.
        let current_tpidr: u64;
        unsafe {
            core::arch::asm!("mrs {}, tpidr_el0", out(reg) current_tpidr);
        }
        // Also update the stored field so switch_task restores it correctly.
        self.tpidr_el0.store(current_tpidr);

        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )?;

        let sp = unsafe {
            let kernel_sp = kernel_stack.as_vaddr().add(KERNEL_STACK_SIZE);
            let mut sp: *mut u64 = kernel_sp.as_mut_ptr();

            // Push a PtRegs frame with parent's register state.
            sp = sp.sub(34);
            let child_frame = sp as *mut u64;

            // Copy x0-x30 from parent.
            for i in 0..31 {
                *child_frame.add(i) = frame.regs[i];
            }
            *child_frame.add(31) = frame.sp;      // sp_el0
            *child_frame.add(32) = frame.pc;      // elr_el1
            *child_frame.add(33) = frame.pstate;  // spsr_el1

            // Context matching do_switch_thread's save layout (low to high):
            //   NZCV, x29, x30, x27, x28, x25, x26, x23, x24, x21, x22, x19, x20
            // push_stack grows downward, so push in reverse (high to low).
            sp = push_stack(sp, frame.regs[20]); // x20
            sp = push_stack(sp, frame.regs[19]); // x19
            sp = push_stack(sp, frame.regs[22]); // x22
            sp = push_stack(sp, frame.regs[21]); // x21
            sp = push_stack(sp, frame.regs[24]); // x24
            sp = push_stack(sp, frame.regs[23]); // x23
            sp = push_stack(sp, frame.regs[26]); // x26
            sp = push_stack(sp, frame.regs[25]); // x25
            sp = push_stack(sp, frame.regs[28]); // x28
            sp = push_stack(sp, frame.regs[27]); // x27
            sp = push_stack(sp, forked_child_entry as *const u8 as u64); // x30 (LR)
            sp = push_stack(sp, frame.regs[29]); // x29 (FP)
            sp = push_stack(sp, 0); // NZCV
            sp
        };

        let interrupt_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )?;
        let syscall_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )?;

        Ok(ArchTask {
            sp: UnsafeCell::new(sp as u64),
            tpidr_el0: AtomicCell::new(current_tpidr),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        })
    }

    /// Stub: ARM64 stacks are freed when ArchTask drops (OwnedPages auto-frees).
    /// This is a no-op placeholder for interface parity with x86_64.
    ///
    /// SAFETY: same contract as x64 — caller guarantees task is off all CPUs.
    #[allow(unsafe_code)]
    pub unsafe fn release_stacks(&self) {
        // OwnedPages frees itself on drop; no Option<> wrapper needed.
    }

    /// Returns the current TLS base (TPIDR_EL0) value.
    pub fn fsbase(&self) -> u64 {
        self.tpidr_el0.load()
    }

    /// Read the saved-by-do_switch_thread context frame.
    ///
    /// Cross-arch API parity with x86_64: the tuple is (saved_sp, saved_pc,
    /// saved_fp) — i.e. (SP, LR/PC, x29) on ARM64.  The layout depends on
    /// exactly what `do_switch_thread` stores on the kernel stack; reading
    /// it safely requires an ARM64-specific port of the x86_64 introspection.
    /// Until that's done, return `None` so the corruption detector in
    /// `kernel/process/process.rs::scan_corrupt_tasks` skips this task
    /// rather than reporting garbage.
    pub fn saved_context_summary(&self) -> Option<(u64, u64, u64)> {
        None
    }

    /// Returns the physical base of the kernel stack (for diagnostics).
    pub fn kernel_stack_paddr(&self) -> Option<PAddr> {
        Some(*self.kernel_stack)
    }

    /// Returns the label of the kernel stack that contains `vaddr`, or None.
    /// Used by the corruption detector to distinguish "saved SP points into
    /// a stack we own" from "saved SP is garbage / dangling pointer".
    pub fn rsp_in_owned_stack(&self, vaddr: u64) -> Option<&'static str> {
        const KERNEL_BASE: u64 = super::KERNEL_BASE_ADDR as u64;
        for (label, stack, num_pages) in [
            ("kernel_stack", &self.kernel_stack, KERNEL_STACK_SIZE),
            ("interrupt_stack", &self.interrupt_stack, AUX_STACK_SIZE),
            ("syscall_stack", &self.syscall_stack, AUX_STACK_SIZE),
        ] {
            let base = (**stack).value() as u64 + KERNEL_BASE;
            if vaddr >= base && vaddr < base + num_pages as u64 {
                return Some(label);
            }
        }
        None
    }

    /// Creates a new thread's arch state.
    /// `child_stack` is the user SP; `tpidr_el0_val` is the TLS base.
    /// x0 = 0 in the child (clone returns 0).
    ///
    /// When `child_stack == 0`, the child inherits the parent's userspace SP.
    /// This matches vfork semantics — the child runs on the parent's stack
    /// until it execs or _exits.  Without this fallback, vfork's child would
    /// start with sp_el0 = 0 and segfault on the first stack push.
    pub fn new_thread(frame: &PtRegs, child_stack: u64, tpidr_el0_val: u64) -> Result<ArchTask, PageAllocError> {
        let child_stack = if child_stack == 0 { frame.sp } else { child_stack };
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )?;

        let sp = unsafe {
            let kernel_sp = kernel_stack.as_vaddr().add(KERNEL_STACK_SIZE);
            let mut sp: *mut u64 = kernel_sp.as_mut_ptr();

            // Push a PtRegs frame, same layout as fork() but with child's stack.
            sp = sp.sub(34);
            let child_frame = sp as *mut u64;
            for i in 0..31 {
                *child_frame.add(i) = frame.regs[i];
            }
            *child_frame.add(0) = 0;           // x0 = 0 (clone returns 0 in child)
            *child_frame.add(31) = child_stack; // sp_el0 = child stack
            *child_frame.add(32) = frame.pc;    // elr_el1 = return PC
            *child_frame.add(33) = frame.pstate; // spsr_el1

            // do_switch_thread context frame.
            sp = push_stack(sp, frame.regs[20]);
            sp = push_stack(sp, frame.regs[19]);
            sp = push_stack(sp, frame.regs[22]);
            sp = push_stack(sp, frame.regs[21]);
            sp = push_stack(sp, frame.regs[24]);
            sp = push_stack(sp, frame.regs[23]);
            sp = push_stack(sp, frame.regs[26]);
            sp = push_stack(sp, frame.regs[25]);
            sp = push_stack(sp, frame.regs[28]);
            sp = push_stack(sp, frame.regs[27]);
            sp = push_stack(sp, forked_child_entry as *const u8 as u64); // x30 (LR)
            sp = push_stack(sp, frame.regs[29]); // x29 (FP)
            sp = push_stack(sp, 0); // NZCV
            sp
        };

        let interrupt_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )?;
        let syscall_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )?;

        Ok(ArchTask {
            sp: UnsafeCell::new(sp as u64),
            tpidr_el0: AtomicCell::new(tpidr_el0_val),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        })
    }

    pub fn setup_execve_stack(
        &self,
        frame: &mut PtRegs,
        ip: UserVAddr,
        user_sp: UserVAddr,
    ) {
        frame.pc = ip.as_isize() as u64;
        frame.sp = user_sp.as_isize() as u64;

        // Reset tpidr_el0 to 0 for the new process.
        //
        // SAVE_REGS/RESTORE_REGS in trap.S do not save or restore tpidr_el0,
        // so the hardware register persists across syscalls.  After fork, the
        // child inherits the parent's TLS base (written by switch_task before
        // the first scheduling).  execve replaces the address space, so the
        // parent's TLS pointer is now stale — musl's __init_tp will set the
        // correct value via `msr tpidr_el0` before any TLS access.
        //
        // We zero both the hardware register (visible when eret returns to the
        // new process entry point) and the stored ArchTask field (so the first
        // context switch restores 0 instead of the parent's stale value).
        #[allow(unsafe_code)]
        unsafe {
            core::arch::asm!("msr tpidr_el0, xzr", options(nomem, nostack));
        }
        self.tpidr_el0.store(0);
    }

    pub fn setup_signal_stack(
        &self,
        frame: &mut PtRegs,
        signal: i32,
        sa_handler: UserVAddr,
        restorer: Option<UserVAddr>,
        _saved_sigmask: u64,
        _original_rsp: u64,
    ) -> Result<usize, AccessError> {
        let mut user_sp = UserVAddr::new_nonnull(frame.sp as usize)?;

        // Determine the LR (return address) for the signal handler.
        // Prefer SA_RESTORER (e.g. musl's __restore_rt) to avoid placing
        // executable code on a non-executable stack.
        let return_pc = if let Some(res) = restorer {
            res.as_isize() as u64
        } else {
            // ARM64 signal trampoline: SVC #0 with x8 = __NR_rt_sigreturn (139).
            const TRAMPOLINE: &[u8] = &[
                0x88, 0x11, 0x80, 0xd2, // mov x8, #139 (__NR_rt_sigreturn)
                0x01, 0x00, 0x00, 0xd4, // svc #0
            ];
            user_sp = user_sp.sub(TRAMPOLINE.len());
            let trampoline_pc = user_sp;
            user_sp.write_bytes(TRAMPOLINE)?;
            trampoline_pc.as_isize() as u64
        };

        // Set x30 (LR) so the signal handler returns to the restorer/trampoline.
        frame.regs[30] = return_pc;

        frame.pc = sa_handler.as_isize() as u64;
        frame.sp = user_sp.as_isize() as u64;
        frame.regs[0] = signal as u64;    // int signal (first argument)
        frame.regs[1] = 0;               // siginfo_t *siginfo
        frame.regs[2] = 0;               // void *ctx

        Ok(0) // ARM64 doesn't use user-stack context save (yet)
    }

    pub fn setup_sigreturn_stack(&self, current_frame: &mut PtRegs, signaled_frame: &PtRegs, _ctx_base: usize) -> u64 {
        *current_frame = *signaled_frame;
        0 // TODO: ARM64 signal mask save/restore on user stack
    }
}

/// Switch from `prev` task to `next` task (ARM64).
///
/// Updates the kernel stack pointer and TLS base, then calls the
/// assembly context switch routine.
pub fn switch_task(prev: &ArchTask, next: &ArchTask) {
    let head = cpu_local_head();

    // Set kernel stack for next thread's exception entry.
    head.sp_el1 = (next.syscall_stack.as_vaddr().value() + AUX_STACK_SIZE) as u64;

    // Save the current (prev) task's TPIDR_EL0 before switching away.
    // User processes can write TPIDR_EL0 directly via `msr tpidr_el0`
    // without going through a syscall, so the ArchTask field may be stale.
    // Reading the hardware register here keeps the field in sync so that
    // fork() copies the correct TLS base.
    //
    // Restore next thread's TPIDR_EL0 (user TLS base).
    unsafe {
        let prev_tpidr: u64;
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) prev_tpidr);
        prev.tpidr_el0.store(prev_tpidr);
        core::arch::asm!("msr tpidr_el0, {}", in(reg) next.tpidr_el0.load());
        // Signal that prev's SP is about to be overwritten.  The assembly
        // sets this back to true after the save, allowing resume() to enqueue
        // the thread without loading a stale SP.
        prev.context_saved.store(false, Ordering::Release);
        do_switch_thread(
            prev.sp.get(),
            next.sp.get(),
            prev.context_saved.as_ptr() as *mut u8,
        );
    }
}

/// Set the ARM64 TLS base register (TPIDR_EL0).
pub fn write_tls_base(value: u64) {
    unsafe {
        core::arch::asm!("msr tpidr_el0, {}", in(reg) value);
    }
}
