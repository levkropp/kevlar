// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Architecture-specific task (process) context for x86_64.
//!
//! This module was moved from kernel/arch/x64/process.rs to consolidate
//! all unsafe code in the platform crate.
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::address::{AccessError, UserVAddr, VAddr};
use crate::page_allocator::{alloc_pages_owned, AllocPageFlags, OwnedPages};
use crate::arch::PAGE_SIZE;
use crate::arch::x64_specific::{cpu_local_head, TSS, USER_CS64, USER_DS, USER_RPL};
use crate::arch::PtRegs;
use crossbeam::atomic::AtomicCell;
use x86::current::segmentation::wrfsbase;

/// Kernel stack size: 256 pages = 1 MiB.
/// Kernel stack size per thread. Linux uses 8-16KB; we use 32KB (8 pages)
/// which is generous for Rust's deeper call stacks. Was 1MB (256 pages)
/// which dominated fork latency with 3 * 1MB allocations per fork.
pub const KERNEL_STACK_SIZE: usize = PAGE_SIZE * 4;

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
    kernel_stack: OwnedPages,
    // FIXME: Do we really need these stacks?
    interrupt_stack: OwnedPages,
    syscall_stack: OwnedPages,
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
        let interrupt_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate interrupt stack");
        let syscall_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate syscall stack");
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");

        let rsp = unsafe {
            let mut rsp: *mut u64 = sp.as_mut_ptr();

            // Registers to be restored in kthread_entry().
            rsp = push_stack(rsp, ip.value() as u64); // The entry point.

            // Registers to be restored in do_switch_thread().
            rsp = push_stack(rsp, kthread_entry as *const u8 as u64); // RIP.
            rsp = push_stack(rsp, 0); // Initial RBP.
            rsp = push_stack(rsp, 0); // Initial RBX.
            rsp = push_stack(rsp, 0); // Initial R12.
            rsp = push_stack(rsp, 0); // Initial R13.
            rsp = push_stack(rsp, 0); // Initial R14.
            rsp = push_stack(rsp, 0); // Initial R15.
            rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled).

            rsp
        };

        ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(0),
            xsave_area: None,
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
    }

    pub fn new_user_thread(ip: UserVAddr, sp: UserVAddr) -> ArchTask {
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
        let interrupt_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
        let syscall_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
        let xsave_area =
            alloc_pages_owned(1, AllocPageFlags::KERNEL).expect("failed to allocate xsave area");
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
            rsp = push_stack(rsp, userland_entry as *const u8 as u64); // RIP.
            rsp = push_stack(rsp, 0); // Initial RBP.
            rsp = push_stack(rsp, 0); // Initial RBX.
            rsp = push_stack(rsp, 0); // Initial R12.
            rsp = push_stack(rsp, 0); // Initial R13.
            rsp = push_stack(rsp, 0); // Initial R14.
            rsp = push_stack(rsp, 0); // Initial R15.
            rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled).

            rsp
        };

        ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(0),
            xsave_area: Some(xsave_area),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
    }

    pub fn new_idle_thread() -> ArchTask {
        let interrupt_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
        let syscall_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocat kernel stack");

        ArchTask {
            rsp: UnsafeCell::new(0),
            fsbase: AtomicCell::new(0),
            xsave_area: None,
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
    }

    pub fn fork(&self, frame: &PtRegs) -> ArchTask {
        let xsave_area =
            alloc_pages_owned(1, AllocPageFlags::KERNEL).expect("failed to allocate xsave area");
        // Copy the parent's FPU/SSE state to the child.
        if let Some(parent_xsave) = self.xsave_area.as_ref() {
            unsafe {
                core::ptr::copy_nonoverlapping::<u8>(
                    parent_xsave.as_mut_ptr(),
                    xsave_area.as_mut_ptr(),
                    PAGE_SIZE,
                );
            }
        } else {
            init_xsave_area(&xsave_area);
        }
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
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
            rsp = push_stack(rsp, forked_child_entry as *const u8 as u64); // RIP.
            rsp = push_stack(rsp, frame.rbp); // UserRBP.
            rsp = push_stack(rsp, frame.rbx); // UserRBX.
            rsp = push_stack(rsp, frame.r12); // UserR12.
            rsp = push_stack(rsp, frame.r13); // UserR13.
            rsp = push_stack(rsp, frame.r14); // UserR14.
            rsp = push_stack(rsp, frame.r15); // UserR15.
            rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled).

            rsp
        };

        // Interrupt and syscall stacks only need enough space for the initial
        // register save before switching to the main kernel stack. 2 pages (8KB)
        // is sufficient (matches Linux's IST stack size).
        let interrupt_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed allocate interrupt stack");
        let syscall_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed allocate syscall stack");

        ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(self.fsbase.load()),
            xsave_area: Some(xsave_area),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
    }

    /// Returns the current FS base register value (TLS base for x86_64).
    pub fn fsbase(&self) -> u64 {
        self.fsbase.load()
    }

    /// Creates a new thread's arch state. Similar to fork() but:
    /// - Uses `child_stack` as the user-space RSP.
    /// - Uses `fs_base` for the FS segment (TLS).
    /// - RAX = 0 in the child (clone returns 0 in child thread).
    pub fn new_thread(frame: &PtRegs, child_stack: u64, fs_base: u64) -> ArchTask {
        let xsave_area =
            alloc_pages_owned(1, AllocPageFlags::KERNEL).expect("failed to allocate xsave area");
        init_xsave_area(&xsave_area);
        let kernel_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate kernel stack");
        let interrupt_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate interrupt stack");
        let syscall_stack = alloc_pages_owned(
            KERNEL_STACK_SIZE / PAGE_SIZE,
            AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK,
        )
        .expect("failed to allocate syscall stack");

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
            rsp = push_stack(rsp, forked_child_entry as *const u8 as u64);
            rsp = push_stack(rsp, frame.rbp);
            rsp = push_stack(rsp, frame.rbx);
            rsp = push_stack(rsp, frame.r12);
            rsp = push_stack(rsp, frame.r13);
            rsp = push_stack(rsp, frame.r14);
            rsp = push_stack(rsp, frame.r15);
            rsp = push_stack(rsp, 0x02); // RFLAGS (interrupts disabled)
            rsp
        };

        ArchTask {
            rsp: UnsafeCell::new(rsp as u64),
            fsbase: AtomicCell::new(fs_base),
            xsave_area: Some(xsave_area),
            interrupt_stack,
            syscall_stack,
            context_saved: AtomicBool::new(true),
            kernel_stack,
        }
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

    pub fn setup_signal_stack(
        &self,
        frame: &mut PtRegs,
        signal: i32,
        sa_handler: UserVAddr,
        restorer: Option<UserVAddr>,
    ) -> Result<(), AccessError> {
        fn push_to_user_stack(rsp: UserVAddr, value: u64) -> Result<UserVAddr, AccessError> {
            let rsp = rsp.sub(8);
            rsp.write::<u64>(&value)?;
            Ok(rsp)
        }

        // Prepare the sigreturn stack in the userspace.
        let mut user_rsp = UserVAddr::new_nonnull(frame.rsp as usize)?;

        // Avoid corrupting the red zone.
        user_rsp = user_rsp.sub(128);

        // Determine the return address for the signal handler:
        // If the caller provided SA_RESTORER (e.g. musl's __restore_rt), use it —
        // it lives in executable text and calls rt_sigreturn for us.
        // Otherwise fall back to writing a small trampoline on the stack.
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

        user_rsp = push_to_user_stack(user_rsp, return_rip)?;

        frame.rip = sa_handler.as_isize() as u64;
        frame.rsp = user_rsp.as_isize() as u64;
        frame.rdi = signal as u64; // int signal
        frame.rsi = 0; // siginfo_t *siginfo
        frame.rdx = 0; // void *ctx

        Ok(())
    }

    pub fn setup_sigreturn_stack(&self, current_frame: &mut PtRegs, signaled_frame: &PtRegs) {
        *current_frame = *signaled_frame;
    }
}

/// Switch from `prev` task to `next` task (x86_64).
///
/// Saves and restores kernel stacks, XSAVE state, FS base, and calls
/// the assembly context switch routine.
pub fn switch_task(prev: &ArchTask, next: &ArchTask) {
    let head = cpu_local_head();

    // Switch the kernel stack.
    head.rsp0 = (next.syscall_stack.as_vaddr().value() + KERNEL_STACK_SIZE) as u64;
    TSS.as_mut()
        .set_rsp0((next.interrupt_stack.as_vaddr().value() + KERNEL_STACK_SIZE) as u64);

    // Save and restore the XSAVE area (i.e. XMM/YMM registers).
    unsafe {
        use core::arch::x86_64::{_xrstor64, _xsave64};

        let xsave_mask = x86::controlregs::xcr0().bits();
        if let Some(xsave_area) = prev.xsave_area.as_ref() {
            _xsave64(xsave_area.as_mut_ptr(), xsave_mask);
        }
        if let Some(xsave_area) = next.xsave_area.as_ref() {
            _xrstor64(xsave_area.as_mut_ptr(), xsave_mask);
        }
    }

    // Fill an invalid value for now: must be initialized in interrupt handlers.
    head.rsp3 = 0xbaad_5a5a_5b5b_baad;

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
    }
}

/// Set the x86_64 FS base register (wrfsbase instruction).
pub fn write_fsbase(value: u64) {
    unsafe {
        wrfsbase(value);
    }
}
