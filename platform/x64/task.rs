// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Architecture-specific task (process) context for x86_64.
//!
//! This module was moved from kernel/arch/x64/process.rs to consolidate
//! all unsafe code in the platform crate.
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::address::{AccessError, PAddr, UserVAddr, VAddr};
use crate::page_allocator::{alloc_pages_owned, AllocPageFlags, OwnedPages, PageAllocError};
use crate::arch::PAGE_SIZE;
use crate::arch::x64_specific::{cpu_local_head, TSS, USER_CS64, USER_DS, USER_RPL};
use crate::arch::PtRegs;
use crossbeam::atomic::AtomicCell;
use x86::current::segmentation::wrfsbase;

/// Kernel stack size per thread.
///
/// Linux x86_64 uses 16KB (4 pages). Rust's deeper call stacks (unwinding,
/// generics, iterator chains) need at least 16KB. 8KB works for simple
/// syscalls but overflows under heavy fork load (200+ simultaneous children).
/// 4KB is too small — immediate overflow in deep paths.
///
/// With per-CPU stack caching, allocation cost is amortized regardless of
/// size, so the bigger stacks don't hurt fork latency.
pub const KERNEL_STACK_SIZE: usize = PAGE_SIZE * 8;

/// End of the user virtual address allocation region.
pub const USER_VALLOC_END: UserVAddr = unsafe { UserVAddr::new_unchecked(0x0000_0fff_0000_0000) };

/// Start of the user virtual address allocation region.
pub const USER_VALLOC_BASE: UserVAddr = unsafe { UserVAddr::new_unchecked(0x0000_000a_0000_0000) };

/// Top of the user stack (grows downward from USER_VALLOC_BASE).
pub const USER_STACK_TOP: UserVAddr = USER_VALLOC_BASE;

/// Architecture-specific process/task context for x86_64.
///
/// Contains the kernel stack pointer, FPU state, and allocated stacks.
pub struct ArchTask {
    rsp: UnsafeCell<u64>,
    pub fsbase: AtomicCell<u64>,
    pub xsave_area: Option<OwnedPages>,
    /// Set to `false` just before `do_switch_thread` saves this task's RSP,
    /// then back to `true` by the assembly after the save.  `resume()` spins
    /// on this flag before enqueuing the task, preventing another CPU from
    /// loading a stale RSP while the save is in flight.
    pub context_saved: AtomicBool,
    // This appears dead, but really we're keeping the pages referenced from the
    // rsp from being dropped until the ArchTask is dropped.
    #[allow(dead_code)]
    kernel_stack: Option<OwnedPages>,
    interrupt_stack: Option<OwnedPages>,
    syscall_stack: Option<OwnedPages>,
}

impl ArchTask {
    /// Physical base address of this task's syscall stack (for debugging).
    pub fn syscall_stack_paddr(&self) -> Option<PAddr> {
        self.syscall_stack.as_ref().map(|s| **s)
    }

    /// Returns true if the kernel stack has been freed (release_stacks was called).
    pub fn kernel_stack_is_none(&self) -> bool {
        self.kernel_stack.is_none()
    }

    /// Read the saved-by-do_switch_thread context frame at the task's stored
    /// RSP. Returns Some((saved_rsp, saved_rip, saved_rbp)) on success,
    /// or None if not safely readable / data is racing under us.
    pub fn saved_context_summary(&self) -> Option<(u64, u64, u64)> {
        use core::sync::atomic::Ordering;
        // Only trust the saved context when do_switch_thread has finished
        // writing it.
        if !self.context_saved.load(Ordering::Acquire) {
            return None;
        }
        let rsp = unsafe { *self.rsp.get() };
        if rsp == 0 || rsp < 0xffff_8000_0000_0000 {
            return None;
        }
        // Double-read with a fence in between — if the task is actually
        // running on another CPU (state racing), the stack content
        // changes between reads. Only return if BOTH reads agree.
        unsafe {
            let rip1 = *((rsp + 7 * 8) as *const u64);
            let rbp1 = *(rsp as *const u64);
            core::sync::atomic::fence(Ordering::SeqCst);
            // Re-check rsp didn't move (another switch happened) and
            // re-check we still trust context_saved.
            let rsp2 = *self.rsp.get();
            if rsp2 != rsp { return None; }
            if !self.context_saved.load(Ordering::Acquire) { return None; }
            let rip2 = *((rsp + 7 * 8) as *const u64);
            let rbp2 = *(rsp as *const u64);
            if rip1 != rip2 || rbp1 != rbp2 {
                // Stack content changed between reads — task is racing.
                return None;
            }
            Some((rsp, rip2, rbp2))
        }
    }

    /// Returns the physical base of the kernel stack (for diagnostics).
    pub fn kernel_stack_paddr(&self) -> Option<PAddr> {
        self.kernel_stack.as_ref().map(|s| **s)
    }

    /// Returns true if the given kernel VA is within any of this task's
    /// allocated stacks (kernel/interrupt/syscall). Used by the
    /// corruption detector to distinguish "saved RSP points into a
    /// stack we own" from "saved RSP is garbage / dangling pointer".
    pub fn rsp_in_owned_stack(&self, vaddr: u64) -> Option<&'static str> {
        const KERNEL_BASE: u64 = 0xffff_8000_0000_0000;
        for (label, stack, num_pages) in [
            ("kernel_stack", self.kernel_stack.as_ref(), KERNEL_STACK_SIZE),
            ("interrupt_stack", self.interrupt_stack.as_ref(), 2 * 4096),
            ("syscall_stack", self.syscall_stack.as_ref(), 2 * 4096),
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

    /// Eagerly release kernel stacks back to the allocator.
    ///
    /// Called from `switch()` after context-switching away from an exiting task.
    /// At that point the task's stacks are no longer in use on any CPU, so it is
    /// safe to free them immediately — matching Linux's `finish_task_switch()` →
    /// `put_task_stack()` pattern.  Without this, zombie processes hold 32 KB of
    /// kernel stacks until wait4() + gc_exited_processes(), causing OOM under
    /// heavy fork/exit workloads (e.g. `apk update`).
    ///
    /// SAFETY: caller must guarantee this task is no longer executing on any CPU.
    #[allow(unsafe_code)]
    pub unsafe fn release_stacks(&self) {
        let this = self as *const Self as *mut Self;
        unsafe {
            if let Some(stack) = (*this).kernel_stack.take() {
                crate::stack_cache::free_kernel_stack(stack, KERNEL_STACK_SIZE / PAGE_SIZE);
            }
            if let Some(stack) = (*this).interrupt_stack.take() {
                crate::stack_cache::free_kernel_stack(stack, 2);
            }
            if let Some(stack) = (*this).syscall_stack.take() {
                crate::stack_cache::free_kernel_stack(stack, 2);
            }
        }
    }

}

impl Drop for ArchTask {
    fn drop(&mut self) {
        // Return stacks to the per-CPU cache instead of freeing via buddy.
        // Cached stacks stay warm in L1/L2, making the next fork faster.
        // Stacks may already be None if release_stacks() was called earlier.
        if let Some(stack) = self.kernel_stack.take() {
            crate::stack_cache::free_kernel_stack(stack, KERNEL_STACK_SIZE / PAGE_SIZE);
        }
        if let Some(stack) = self.interrupt_stack.take() {
            crate::stack_cache::free_kernel_stack(stack, 2);
        }
        if let Some(stack) = self.syscall_stack.take() {
            crate::stack_cache::free_kernel_stack(stack, 2);
        }
    }
}

unsafe impl Sync for ArchTask {}

/// Initialize an xsave area with valid default FPU/SSE state.
///
/// The XSAVE legacy region (bytes 0-511) mirrors the FXSAVE layout:
/// - Offset 0: FCW (x87 control word) = 0x037F (default)
/// - Offset 24: MXCSR = 0x1F80 (all SSE exceptions masked)
///
/// Without this, a zeroed xsave area has MXCSR=0 (all exceptions unmasked),
/// causing SIMD_FLOATING_POINT (#XM) on the first SSE operation.
#[allow(unsafe_code)]
fn init_xsave_area(xsave: &OwnedPages) {
    unsafe {
        let ptr = xsave.as_mut_ptr::<u8>();
        // FCW: x87 FPU control word — mask all x87 exceptions
        *(ptr.add(0) as *mut u16) = 0x037F;
        // MXCSR: SSE control/status — mask all SSE exceptions
        *(ptr.add(24) as *mut u32) = 0x1F80;
    }
}

unsafe extern "C" {
    fn kthread_entry();
    fn userland_entry();
    fn forked_child_entry();
    fn do_switch_thread(prev_rsp: *mut u64, next_rsp: *const u64, ctx_saved: *mut u8);
}

unsafe fn push_stack(mut rsp: *mut u64, value: u64) -> *mut u64 {
    unsafe {
        rsp = rsp.sub(1);
        rsp.write(value);
    }
    rsp
}

impl ArchTask {
    #[allow(unused)]
    pub fn new_kthread(ip: VAddr, sp: VAddr) -> ArchTask {
        let interrupt_stack = crate::stack_cache::alloc_kernel_stack(2).expect("kthread IST stack");
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(2).expect("kthread syscall stack");
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE).expect("kthread kernel stack");

        let rsp = unsafe {
            let mut rsp: *mut u64 = sp.as_mut_ptr();

            // Registers to be restored in kthread_entry().
            rsp = push_stack(rsp, ip.value() as u64); // The entry point.

            // Registers to be restored in do_switch_thread().
            // Order: pop rbp, rbx, r12, r13, r14, r15, popfq, ret
            rsp = push_stack(rsp, kthread_entry as *const u8 as u64); // RIP.
            rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled).
            rsp = push_stack(rsp, 0); // Initial R15.
            rsp = push_stack(rsp, 0); // Initial R14.
            rsp = push_stack(rsp, 0); // Initial R13.
            rsp = push_stack(rsp, 0); // Initial R12.
            rsp = push_stack(rsp, 0); // Initial RBX.
            rsp = push_stack(rsp, 0); // Initial RBP.

            rsp
        };

        ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(0),
            xsave_area: None,
            interrupt_stack: Some(interrupt_stack),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
        }
    }

    pub fn new_user_thread(ip: UserVAddr, sp: UserVAddr) -> ArchTask {
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE).expect("user thread kernel stack");
        let interrupt_stack = crate::stack_cache::alloc_kernel_stack(2).expect("user thread IST stack");
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(2).expect("user thread syscall stack");
        let xsave_area =
            alloc_pages_owned(2, AllocPageFlags::KERNEL).expect("user thread xsave area");
        init_xsave_area(&xsave_area);

        let rsp = unsafe {
            let kernel_sp = kernel_stack.as_vaddr().add(KERNEL_STACK_SIZE);
            let mut rsp: *mut u64 = kernel_sp.as_mut_ptr();

            // Registers to be restored by IRET.
            rsp = push_stack(rsp, (USER_DS | USER_RPL) as u64); // SS
            rsp = push_stack(rsp, sp.value() as u64); // user RSP
            rsp = push_stack(rsp, 0x202); // RFLAGS (interrupts enabled).
            rsp = push_stack(rsp, (USER_CS64 | USER_RPL) as u64); // CS
            rsp = push_stack(rsp, ip.value() as u64); // RIP

            // Registers to be restored in do_switch_thread().
            // Order: pop rbp, rbx, r12, r13, r14, r15, popfq, ret
            rsp = push_stack(rsp, userland_entry as *const u8 as u64); // RIP.
            rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled).
            rsp = push_stack(rsp, 0); // Initial R15.
            rsp = push_stack(rsp, 0); // Initial R14.
            rsp = push_stack(rsp, 0); // Initial R13.
            rsp = push_stack(rsp, 0); // Initial R12.
            rsp = push_stack(rsp, 0); // Initial RBX.
            rsp = push_stack(rsp, 0); // Initial RBP.

            rsp
        };

        ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(0),
            xsave_area: Some(xsave_area),
            interrupt_stack: Some(interrupt_stack),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
        }
    }

    pub fn new_idle_thread() -> ArchTask {
        let interrupt_stack = crate::stack_cache::alloc_kernel_stack(2).expect("idle thread IST stack");
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(2).expect("idle thread syscall stack");
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE).expect("idle thread kernel stack");

        ArchTask {
            rsp: UnsafeCell::new(0),
            fsbase: AtomicCell::new(0),
            xsave_area: None,
            interrupt_stack: Some(interrupt_stack),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
        }
    }

    pub fn fork(&self, frame: &PtRegs) -> Result<ArchTask, PageAllocError> {
        let xsave_area = alloc_pages_owned(2, AllocPageFlags::KERNEL)?;
        // Copy the parent's FPU/SSE state to the child.
        if let Some(parent_xsave) = self.xsave_area.as_ref() {
            unsafe {
                core::ptr::copy_nonoverlapping::<u8>(
                    parent_xsave.as_mut_ptr(),
                    xsave_area.as_mut_ptr(),
                    2 * PAGE_SIZE,
                );
            }
        } else {
            init_xsave_area(&xsave_area);
        }
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE)?;
        let rsp = unsafe {
            let kernel_sp = kernel_stack.as_vaddr().add(KERNEL_STACK_SIZE);
            let mut rsp: *mut u64 = kernel_sp.as_mut_ptr();

            // Registers to be restored by IRET.
            rsp = push_stack(rsp, (USER_DS | USER_RPL) as u64); // SS
            rsp = push_stack(rsp, frame.rsp); // user RSP
            rsp = push_stack(rsp, frame.rflags); // user RFLAGS.
            rsp = push_stack(rsp, (USER_CS64 | USER_RPL) as u64); // CS
            rsp = push_stack(rsp, frame.rip); // user RIP

            // Registers to be restored in forked_child_entry,
            rsp = push_stack(rsp, frame.rflags); // user R11
            rsp = push_stack(rsp, frame.rip); // user RCX
            rsp = push_stack(rsp, frame.r10);
            rsp = push_stack(rsp, frame.r9);
            rsp = push_stack(rsp, frame.r8);
            rsp = push_stack(rsp, frame.rsi);
            rsp = push_stack(rsp, frame.rdi);
            rsp = push_stack(rsp, frame.rdx);

            // Registers to be restored in do_switch_thread().
            // Order must match do_switch_thread's pop sequence:
            //   pop rbp, rbx, r12, r13, r14, r15, popfq, ret
            rsp = push_stack(rsp, forked_child_entry as *const u8 as u64); // RIP (popped by ret).
            rsp = push_stack(rsp, 0x02); // RFLAGS (popped by popfq, IF=0).
            rsp = push_stack(rsp, frame.r15);
            rsp = push_stack(rsp, frame.r14);
            rsp = push_stack(rsp, frame.r13);
            rsp = push_stack(rsp, frame.r12);
            rsp = push_stack(rsp, frame.rbx);
            rsp = push_stack(rsp, frame.rbp);

            rsp
        };

        // Interrupt and syscall stacks only need enough space for the initial
        // register save before switching to the main kernel stack. 2 pages (8KB)
        // is sufficient (matches Linux's IST stack size).
        let interrupt_stack = crate::stack_cache::alloc_kernel_stack(2)?;
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(2)?;

        Ok(ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(self.fsbase.load()),
            xsave_area: Some(xsave_area),
            interrupt_stack: Some(interrupt_stack),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
        })
    }

    /// Returns the current FS base register value (TLS base for x86_64).
    pub fn fsbase(&self) -> u64 {
        self.fsbase.load()
    }

    /// Creates a new thread's arch state. Similar to fork() but:
    /// - Uses `child_stack` as the user-space RSP.
    /// - Uses `fs_base` for the FS segment (TLS).
    /// - RAX = 0 in the child (clone returns 0 in child thread).
    pub fn new_thread(frame: &PtRegs, child_stack: u64, fs_base: u64) -> Result<ArchTask, PageAllocError> {
        let xsave_area = alloc_pages_owned(2, AllocPageFlags::KERNEL)?;
        init_xsave_area(&xsave_area);
        let kernel_stack = crate::stack_cache::alloc_kernel_stack(KERNEL_STACK_SIZE / PAGE_SIZE)?;
        let interrupt_stack = crate::stack_cache::alloc_kernel_stack(2)?;
        let syscall_stack = crate::stack_cache::alloc_kernel_stack(2)?;

        let rsp = unsafe {
            let kernel_sp = kernel_stack.as_vaddr().add(KERNEL_STACK_SIZE);
            let mut rsp: *mut u64 = kernel_sp.as_mut_ptr();

            // IRET frame for returning to userspace.
            rsp = push_stack(rsp, (USER_DS | USER_RPL) as u64); // SS
            rsp = push_stack(rsp, child_stack);                  // user RSP
            rsp = push_stack(rsp, frame.rflags);                 // RFLAGS
            rsp = push_stack(rsp, (USER_CS64 | USER_RPL) as u64); // CS
            rsp = push_stack(rsp, frame.rip);                    // RIP (clone return addr)

            // Registers popped by forked_child_entry before IRET.
            rsp = push_stack(rsp, frame.rflags); // r11
            rsp = push_stack(rsp, frame.rip);    // rcx
            rsp = push_stack(rsp, frame.r10);
            rsp = push_stack(rsp, frame.r9);
            rsp = push_stack(rsp, frame.r8);
            rsp = push_stack(rsp, frame.rsi);
            rsp = push_stack(rsp, frame.rdi);
            rsp = push_stack(rsp, frame.rdx);

            // do_switch_thread context frame.
            // Order: pop rbp, rbx, r12, r13, r14, r15, popfq, ret
            rsp = push_stack(rsp, forked_child_entry as *const u8 as u64);
            rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled)
            rsp = push_stack(rsp, frame.r15);
            rsp = push_stack(rsp, frame.r14);
            rsp = push_stack(rsp, frame.r13);
            rsp = push_stack(rsp, frame.r12);
            rsp = push_stack(rsp, frame.rbx);
            rsp = push_stack(rsp, frame.rbp);
            rsp
        };

        Ok(ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(fs_base),
            xsave_area: Some(xsave_area),
            interrupt_stack: Some(interrupt_stack),
            syscall_stack: Some(syscall_stack),
            context_saved: AtomicBool::new(true),
            kernel_stack: Some(kernel_stack),
        })
    }

    pub fn setup_execve_stack(
        &self,
        frame: &mut PtRegs,
        ip: UserVAddr,
        user_sp: UserVAddr,
    ) {
        frame.rip = ip.as_isize() as u64;
        frame.rsp = user_sp.as_isize() as u64;
    }

    /// Set up the user stack for signal handler invocation.
    ///
    /// Saves the complete interrupted context (all GPRs, RIP, RSP, RFLAGS)
    /// and signal mask on the user stack.  `rt_sigreturn` reads them back,
    /// so nested signal delivery works correctly — each signal frame is
    /// independent on the user stack, no kernel-side single-slot limitation.
    /// Returns the ctx_base address on success (needed for sigreturn to find
    /// the saved context, especially when using an alternate signal stack).
    /// `original_rsp`: The user RSP BEFORE the alt stack switch.  Saved into
    /// the signal context so that `rt_sigreturn` restores the original stack.
    pub fn setup_signal_stack(
        &self,
        frame: &mut PtRegs,
        signal: i32,
        sa_handler: UserVAddr,
        restorer: Option<UserVAddr>,
        saved_sigmask: u64,
        original_rsp: u64,
    ) -> Result<usize, AccessError> {
        fn push_to_user_stack(rsp: UserVAddr, value: u64) -> Result<UserVAddr, AccessError> {
            let rsp = rsp.sub(8);
            rsp.write::<u64>(&value)?;
            Ok(rsp)
        }

        let mut user_rsp = UserVAddr::new_nonnull(frame.rsp as usize)?;

        // Red zone (128 bytes below RSP that the function may use).
        user_rsp = user_rsp.sub(128);

        // === Signal context frame (saved on user stack) ===
        // Layout (152 bytes, at the top of the 832-byte reserved area):
        //   [+0]   saved_sigmask  (u64)
        //   [+8]   rip            (u64)
        //   [+16]  rsp            (u64)
        //   [+24]  rbp            (u64)
        //   [+32]  rax            (u64)
        //   [+40]  rbx            (u64)
        //   [+48]  rcx            (u64)
        //   [+56]  rdx            (u64)
        //   [+64]  rsi            (u64)
        //   [+72]  rdi            (u64)
        //   [+80]  r8             (u64)
        //   [+88]  r9             (u64)
        //   [+96]  r10            (u64)
        //   [+104] r11            (u64)
        //   [+112] r12            (u64)
        //   [+120] r13            (u64)
        //   [+128] r14            (u64)
        //   [+136] r15            (u64)
        //   [+144] rflags         (u64)
        //   [+152..832] padding/unused
        user_rsp = user_rsp.sub(832);
        let ctx_base = user_rsp;

        // Write the full context to the user stack.
        // Use a flat array to avoid unaligned-ref errors on packed PtRegs.
        let regs: [u64; 19] = [
            saved_sigmask,
            { frame.rip }, original_rsp, { frame.rbp },
            { frame.rax }, { frame.rbx }, { frame.rcx }, { frame.rdx },
            { frame.rsi }, { frame.rdi },
            { frame.r8  }, { frame.r9  }, { frame.r10 }, { frame.r11 },
            { frame.r12 }, { frame.r13 }, { frame.r14 }, { frame.r15 },
            { frame.rflags },
        ];
        for (i, &val) in regs.iter().enumerate() {
            ctx_base.add(i * 8).write::<u64>(&val)?;
        }

        let return_rip = if let Some(res) = restorer {
            res.as_isize() as u64
        } else {
            const TRAMPOLINE: &[u8] = &[
                0xb8, 0x0f, 0x00, 0x00, 0x00, // mov eax, 15  (__NR_rt_sigreturn)
                0x0f, 0x05,                    // syscall
                0x90,                          // nop (alignment)
            ];
            user_rsp = user_rsp.sub(TRAMPOLINE.len());
            let trampoline_rip = user_rsp;
            user_rsp.write_bytes(TRAMPOLINE)?;
            trampoline_rip.as_isize() as u64
        };

        // 16-byte align RSP (x86_64 ABI).
        let aligned = user_rsp.value() & !0xF;
        user_rsp = UserVAddr::new_nonnull(aligned)?;


        user_rsp = push_to_user_stack(user_rsp, return_rip)?;

        frame.rip = sa_handler.as_isize() as u64;
        frame.rsp = user_rsp.as_isize() as u64;

        // Lock-free verify: read back what we just wrote.
        frame.rdi = signal as u64; // int signal
        frame.rsi = 0; // siginfo_t *siginfo
        frame.rdx = 0; // void *ctx

        Ok(ctx_base.value())
    }

    /// Restore the interrupted context from the user stack.
    /// Reads all saved GPRs, RIP, RSP, RFLAGS, and signal mask from
    /// the signal frame that was written by `setup_signal_stack`.
    /// Returns the saved signal mask for the caller to restore.
    pub fn setup_sigreturn_stack(&self, current_frame: &mut PtRegs, signaled_frame: &PtRegs, ctx_base_val: usize) -> u64 {
        // ctx_base_val was saved by setup_signal_stack and stored in signal_ctx_base_stack.
        // It points to where the register context was written on the user stack
        // (possibly the alternate signal stack).
        let ctx = match UserVAddr::new_nonnull(ctx_base_val) {
            Ok(addr) => addr,
            Err(_) => {
                *current_frame = *signaled_frame;
                return 0;
            }
        };

        // Read all fields from the user stack context frame.
        let saved_mask = ctx.read::<u64>().unwrap_or(0);
        current_frame.rip = ctx.add(8).read::<u64>().unwrap_or(signaled_frame.rip);
        current_frame.rsp = ctx.add(16).read::<u64>().unwrap_or(signaled_frame.rsp);
        current_frame.rbp = ctx.add(24).read::<u64>().unwrap_or(signaled_frame.rbp);
        current_frame.rax = ctx.add(32).read::<u64>().unwrap_or(signaled_frame.rax);
        current_frame.rbx = ctx.add(40).read::<u64>().unwrap_or(signaled_frame.rbx);
        current_frame.rcx = ctx.add(48).read::<u64>().unwrap_or(signaled_frame.rcx);
        current_frame.rdx = ctx.add(56).read::<u64>().unwrap_or(signaled_frame.rdx);
        current_frame.rsi = ctx.add(64).read::<u64>().unwrap_or(signaled_frame.rsi);
        current_frame.rdi = ctx.add(72).read::<u64>().unwrap_or(signaled_frame.rdi);
        current_frame.r8 = ctx.add(80).read::<u64>().unwrap_or(signaled_frame.r8);
        current_frame.r9 = ctx.add(88).read::<u64>().unwrap_or(signaled_frame.r9);
        current_frame.r10 = ctx.add(96).read::<u64>().unwrap_or(signaled_frame.r10);
        current_frame.r11 = ctx.add(104).read::<u64>().unwrap_or(signaled_frame.r11);
        current_frame.r12 = ctx.add(112).read::<u64>().unwrap_or(signaled_frame.r12);
        current_frame.r13 = ctx.add(120).read::<u64>().unwrap_or(signaled_frame.r13);
        current_frame.r14 = ctx.add(128).read::<u64>().unwrap_or(signaled_frame.r14);
        current_frame.r15 = ctx.add(136).read::<u64>().unwrap_or(signaled_frame.r15);
        current_frame.rflags = ctx.add(144).read::<u64>().unwrap_or(signaled_frame.rflags);

        saved_mask
    }
}

/// Switch from `prev` task to `next` task (x86_64).
///
/// Saves and restores kernel stacks, XSAVE state, FS base, and calls
/// the assembly context switch routine.
pub fn switch_task(prev: &ArchTask, next: &ArchTask) {
    let head = cpu_local_head();

    // Switch the kernel stack.
    head.rsp0 = (next.syscall_stack.as_ref().unwrap().as_vaddr().value() + 2 * PAGE_SIZE) as u64;
    TSS.as_mut()
        .set_rsp0((next.interrupt_stack.as_ref().unwrap().as_vaddr().value() + 2 * PAGE_SIZE) as u64);

    // Save and restore the XSAVE area (FPU/SSE/AVX registers).
    // Uses inline asm instead of Rust _xsave64/_xrstor64 intrinsics, which
    // corrupt the kernel stack under the soft-float target (the intrinsics
    // generate SSE-using code that clobbers the stack frame).
    unsafe {
        let xsave_mask = x86::controlregs::xcr0().bits();
        let mask_lo = xsave_mask as u32;
        let mask_hi = (xsave_mask >> 32) as u32;
        if let Some(xsave_area) = prev.xsave_area.as_ref() {
            let ptr = xsave_area.as_mut_ptr::<u8>();
            core::arch::asm!(
                "xsave64 [{}]",
                in(reg) ptr,
                in("eax") mask_lo,
                in("edx") mask_hi,
                options(nostack, preserves_flags),
            );
        }
        if let Some(xsave_area) = next.xsave_area.as_ref() {
            let ptr = xsave_area.as_mut_ptr::<u8>();
            core::arch::asm!(
                "xrstor64 [{}]",
                in(reg) ptr,
                in("eax") mask_lo,
                in("edx") mask_hi,
                options(nostack, preserves_flags),
            );
        }
    }

    // Fill an invalid value for now: must be initialized in interrupt handlers.
    head.rsp3 = 0xbaad_5a5a_5b5b_baad;

    // Sanity check: verify the return address on next's saved stack is valid.
    let next_rsp_val = unsafe { *next.rsp.get() };
    if next_rsp_val != 0 && next_rsp_val >= 0xffff_8000_0000_0000 {
        let ret_addr = unsafe { *((next_rsp_val + 56) as *const u64) };
        if ret_addr == 0 || (ret_addr > 0 && ret_addr < 0xffff_8000_0000_0000) {
            panic!(
                "switch_thread BUG: cpu={} rsp={:#x} ret={:#x}",
                crate::arch::cpu_id(), next_rsp_val, ret_addr,
            );
        }
    }

    unsafe {
        wrfsbase(next.fsbase.load());
        // Signal that prev's RSP is about to be overwritten.  The assembly
        // sets this back to true after the save, allowing resume() to enqueue
        // the thread without loading a stale RSP.
        prev.context_saved.store(false, Ordering::Release);
        do_switch_thread(
            prev.rsp.get(),
            next.rsp.get(),
            prev.context_saved.as_ptr() as *mut u8,
        );
        // CRITICAL: prevent the compiler from tail-calling do_switch_thread.
        // do_switch_thread is a context switch — it returns in a DIFFERENT
        // thread's context.  A tail call tears down this frame first, but the
        // returning thread needs this frame intact to clean up correctly.
        core::arch::asm!("", options(nomem, nostack));
    }
}

/// Set the x86_64 FS base register (wrfsbase instruction).
pub fn write_fsbase(value: u64) {
    unsafe {
        wrfsbase(value);
    }
}
