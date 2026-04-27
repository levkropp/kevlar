// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Architecture-specific task (process) context for ARM64.
//!
//! This module was moved from kernel/arch/arm64/process.rs to consolidate
//! all unsafe code in the platform crate.
use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::address::{AccessError, PAddr, UserVAddr, VAddr};
use crate::page_allocator::{alloc_pages_owned, AllocPageFlags, OwnedPages, PageAllocError};
use crate::arch::PAGE_SIZE;
use crate::arch::arm64_specific::cpu_local_head;
use crate::arch::PtRegs;
use crossbeam::atomic::AtomicCell;

/// FP/NEON register state: v0-v31 (512 B) + FPCR (8 B) + FPSR (8 B) = 528 B.
///
/// Kept per-task and swapped by `do_switch_thread` on context switch.  The
/// kernel itself is compiled `-neon,-fp-armv8` (see `kernel/arch/arm64/
/// arm64.json`) so EL1 code never writes v-regs; FP state therefore survives
/// EL0→EL1→EL0 round-trips naturally, and the old per-exception save/restore
/// in `trap.S` was moved into context-switch.  That matches what Linux arm64
/// does with `-mgeneral-regs-only` + `fpsimd_save_state`/`fpsimd_load_state`.
#[repr(C, align(16))]
pub struct FpState {
    pub v: [u128; 32],
    pub fpcr: u64,
    pub fpsr: u64,
}

impl FpState {
    pub fn zeroed() -> Box<Self> {
        Box::new(FpState {
            v: [0u128; 32],
            fpcr: 0,
            fpsr: 0,
        })
    }
}

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
    // Option<OwnedPages> lets `release_stacks` and `Drop` `.take()` each
    // stack and route it through `stack_cache::free_kernel_stack` instead
    // of the buddy allocator — subsequent forks get warm, pre-sized stacks
    // from the per-size cache (2-page and 8-page classes).
    //
    // Note: x86_64 keeps a separate `interrupt_stack` (IST entry for NMIs
    // and double-faults).  ARM64's exception model routes everything
    // through the single `syscall_stack` (sp_el1), so we don't need one.
    kernel_stack: Option<OwnedPages>,
    syscall_stack: Option<OwnedPages>,
    /// Per-task FP/NEON state (528 B).  Saved/restored *lazily* by the
    /// EC=0x07 FP-trap handler, NOT on context switch — see `FpState` docs.
    fp_state: Box<FpState>,
    /// True when this task's `fp_state` is currently loaded in the v-regs of
    /// some CPU.  Inverse of Linux's `TIF_FOREIGN_FPSTATE` flag.  Cleared
    /// by the trap handler when it saves another task's state out of HW
    /// (this task's state becomes foreign w.r.t. the HW) and by `new_*`
    /// constructors.  Set when the trap handler loads this task's state.
    pub fp_loaded: AtomicBool,
}

unsafe impl Sync for ArchTask {}

impl ArchTask {
    /// Raw pointer to this task's `FpState`.  Used by the EC=0x07 FP-trap
    /// handler to save/restore state directly without going through
    /// `PROCESSES.lock()` (which isn't reachable from the platform crate).
    /// The pointer is stable for the lifetime of the `ArchTask` — the
    /// inner `Box<FpState>` is only moved by `Drop`.
    #[inline(always)]
    pub fn fp_state_ptr(&self) -> *mut FpState {
        &*self.fp_state as *const FpState as *mut FpState
    }
}

impl Drop for ArchTask {
    fn drop(&mut self) {
        // Route remaining stacks through the per-size cache instead of the
        // buddy allocator.  Stacks are None if `release_stacks` ran earlier;
        // the `.take()` pattern is idempotent.
        if let Some(stack) = self.kernel_stack.take() {
            crate::stack_cache::free_kernel_stack(stack, KERNEL_STACK_SIZE / PAGE_SIZE);
        }
        if let Some(stack) = self.syscall_stack.take() {
            crate::stack_cache::free_kernel_stack(stack, AUX_STACK_PAGES);
        }
    }
}

unsafe extern "C" {
    fn kthread_entry();
    fn userland_entry();
    fn forked_child_entry();
    fn do_switch_thread(
        prev_sp: *mut u64,
        next_sp: *const u64,
        ctx_saved: *mut u8,
    );
    /// Snapshot the live hardware FP/NEON state (v0-v31, FPCR, FPSR) into
    /// `dst`.  Used in `fork()` to copy the parent's FP state into the child:
    /// at SVC entry the parent's v-regs are still live in HW because the
    /// trap handler no longer saves/restores them.
    pub fn kevlar_save_fp_to(dst: *mut FpState);
    /// Load FP/NEON state (v0-v31, FPCR, FPSR) from `src` into the live HW.
    /// Used by the EC=0x07 FP-trap handler on first EL0 SIMD use to make the
    /// current task's saved state live.
    pub fn kevlar_restore_fp_from(src: *const FpState);
}

unsafe fn push_stack(mut sp: *mut u64, value: u64) -> *mut u64 {
    unsafe {
        sp = sp.sub(1);
        sp.write(value);
    }
    sp
}

impl ArchTask {
    pub fn new_kthread(ip: VAddr) -> ArchTask {
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(AUX_STACK_PAGES)
            .expect("failed to allocate syscall stack");
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE)
            .expect("failed to allocate kernel stack");

        let stack_top = kernel_stack.as_vaddr().add(KERNEL_STACK_SIZE);

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
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
            fp_state: FpState::zeroed(),
            fp_loaded: AtomicBool::new(false),
        }
    }

    pub fn new_user_thread(ip: UserVAddr, user_sp: UserVAddr) -> ArchTask {
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE)
            .expect("failed to allocate kernel stack");
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(AUX_STACK_PAGES)
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
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
            fp_state: FpState::zeroed(),
            fp_loaded: AtomicBool::new(false),
        }
    }

    pub fn new_idle_thread() -> ArchTask {
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(AUX_STACK_PAGES)
            .expect("failed to allocate syscall stack");
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE)
            .expect("failed to allocate kernel stack");

        ArchTask {
            sp: UnsafeCell::new(0),
            tpidr_el0: AtomicCell::new(0),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
            fp_state: FpState::zeroed(),
            fp_loaded: AtomicBool::new(false),
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

        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE)?;

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

        let syscall_stack = crate::stack_cache::alloc_kernel_stack(AUX_STACK_PAGES)?;

        // Snapshot the parent's live HW FP/NEON state into the child.  At this
        // point we are in EL1 handling the fork SVC; the trap handler no
        // longer saves v-regs, so the parent's FP state is still in the
        // hardware registers.
        let mut fp_state = FpState::zeroed();
        #[allow(unsafe_code)]
        unsafe {
            kevlar_save_fp_to(&mut *fp_state as *mut FpState);
        }

        Ok(ArchTask {
            sp: UnsafeCell::new(sp as u64),
            tpidr_el0: AtomicCell::new(current_tpidr),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
            fp_state,
            fp_loaded: AtomicBool::new(false),
        })
    }

    /// Eagerly return kernel stacks to the per-size cache.
    ///
    /// Called from `switch()` after context-switching away from an exiting
    /// task — the stacks are guaranteed to be off all CPUs at that point.
    /// Routes through `stack_cache::free_kernel_stack` so subsequent forks
    /// can allocate from a warm cache instead of the buddy allocator.
    ///
    /// SAFETY: caller must guarantee this task is no longer executing on any
    /// CPU and no remote CPU is about to resume it.
    #[allow(unsafe_code)]
    pub unsafe fn release_stacks(&self) {
        let this = self as *const Self as *mut Self;
        unsafe {
            if let Some(stack) = (*this).kernel_stack.take() {
                crate::stack_cache::free_kernel_stack(stack, KERNEL_STACK_SIZE / PAGE_SIZE);
            }
            if let Some(stack) = (*this).syscall_stack.take() {
                crate::stack_cache::free_kernel_stack(stack, AUX_STACK_PAGES);
            }
        }
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
        self.kernel_stack.as_ref().map(|s| **s)
    }

    /// Returns the label of the kernel stack that contains `vaddr`, or None.
    /// Used by the corruption detector to distinguish "saved SP points into
    /// a stack we own" from "saved SP is garbage / dangling pointer".
    pub fn rsp_in_owned_stack(&self, vaddr: u64) -> Option<&'static str> {
        const KERNEL_BASE: u64 = super::KERNEL_BASE_ADDR as u64;
        for (label, stack, num_pages) in [
            ("kernel_stack", self.kernel_stack.as_ref(), KERNEL_STACK_SIZE),
            ("syscall_stack", self.syscall_stack.as_ref(), AUX_STACK_SIZE),
        ] {
            if let Some(s) = stack {
                let base = (**s).value() as u64 + KERNEL_BASE;
                if vaddr >= base && vaddr < base + num_pages as u64 {
                    return Some(label);
                }
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
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE)?;

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

        let syscall_stack = crate::stack_cache::alloc_kernel_stack(AUX_STACK_PAGES)?;

        // Snapshot parent's live HW FP/NEON state — same reasoning as `fork`.
        // pthread children inherit v-regs across the clone SVC so that caller-
        // saved FP values in the parent aren't mysteriously zeroed on the
        // first context switch into the child.
        let mut fp_state = FpState::zeroed();
        #[allow(unsafe_code)]
        unsafe {
            kevlar_save_fp_to(&mut *fp_state as *mut FpState);
        }

        Ok(ArchTask {
            sp: UnsafeCell::new(sp as u64),
            tpidr_el0: AtomicCell::new(tpidr_el0_val),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
            fp_state,
            fp_loaded: AtomicBool::new(false),
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
    head.sp_el1 = (next.syscall_stack.as_ref().expect("syscall_stack present")
        .as_vaddr().value() + AUX_STACK_SIZE) as u64;

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
        // Lazy FP: usually arm CPACR_EL1.FPEN to trap EL0 SIMD, so the
        // next task's first FP instruction faults into
        // `fp::handle_fp_trap` and that handler loads its FpState
        // on demand.  Skip the trap arming on the fast path — when the
        // CPU's v-regs already hold `next`'s state (next.fp_loaded &&
        // CPU's fp_owner points at next.fp_state).  This is the
        // common case for fork+wait benches where the parent gets
        // re-scheduled onto the same CPU before any other task uses
        // FP.  Saves a CPACR MSR (cheap on real HW, but can trap to
        // the hypervisor under HVF) plus the subsequent FP-trap.
        let next_fp_ptr = next.fp_state_ptr() as u64;
        if next.fp_loaded.load(Ordering::Acquire)
            && cpu_local_head().fp_owner == next_fp_ptr
        {
            // Same task back on the same CPU; v-regs are already its.
            // Leave CPACR at 0b11.
        } else {
            super::fp::cpacr_trap_el0_fp();
        }
    }
}

/// Set the ARM64 TLS base register (TPIDR_EL0).
pub fn write_tls_base(value: u64) {
    unsafe {
        core::arch::asm!("msr tpidr_el0, {}", in(reg) value);
    }
}
