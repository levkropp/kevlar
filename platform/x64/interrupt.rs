// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{address::{UserVAddr, VAddr}, handler};

use core::fmt;

use super::{apic::{ack_interrupt, LAPIC_PREEMPT_VECTOR, PANIC_HALT_VECTOR, TLB_SHOOTDOWN_VECTOR}, ioapic::VECTOR_IRQ_BASE, serial::SERIAL0_IRQ, PageFaultReason};
use x86::{
    controlregs::cr2,
    current::rflags::{self, RFlags},
    irq::*,
};

/// The interrupt stack frame.
#[derive(Copy, Clone)]
#[repr(C, packed)]
struct InterruptFrame {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rbp: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rdi: u64,
    error: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

impl fmt::Debug for InterruptFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rip = self.rip;
        let rsp = self.rsp;
        let cs = self.cs;
        let error = self.error;
        write!(
            f,
            "RIP={:x}, RSP={:x}, CS={:x}, ERR={:x}",
            rip, rsp, cs, error
        )
    }
}

unsafe extern "C" {
    fn usercopy1();
    fn usercopy1b();
    fn usercopy1c();
    fn usercopy1d();
    fn usercopy2();
    fn usercopy3();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn x64_handle_interrupt(vec: u8, frame: *mut InterruptFrame) {
    let frame = &mut *frame;

    // FIXME: Check "Legacy replacement" mapping
    const TIMER_IRQ: u8 = 0;
    const TIMER_IRQ2: u8 = 2;
    // Interrupt tracing moved behind the debug event system.
    // The old trace!() here wrote to the serial port on every non-timer
    // interrupt, causing ~6 VM exits per interrupt (serial busy-wait +
    // VGA cursor updates).  This was the single largest source of KVM
    // overhead.  Use `debug=irq` at the kernel command line if needed.

    match vec {
        PANIC_HALT_VECTOR => {
            // Another CPU is panicking — halt this CPU immediately.
            // Disable interrupts and spin forever so the panicking CPU has
            // exclusive access to the serial console.
            ack_interrupt();
            unsafe { core::arch::asm!("cli", options(nomem, nostack)); }
            loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
        }
        LAPIC_PREEMPT_VECTOR => {
            ack_interrupt();
            crate::flight_recorder::record(
                crate::flight_recorder::kind::PREEMPT,
                super::cpu_id() as u32,
                0, 0,
            );
            // Trigger a context switch if another thread is ready.
            // Signal delivery for the current thread (which may have changed
            // after the switch) is handled by x64_check_signal_on_irq_return
            // in trap.S, called after this function returns with the correct
            // frame pointer (RSP now points to the current thread's frame).
            crate::handler().handle_ap_preempt();
        }
        TLB_SHOOTDOWN_VECTOR => {
            // A peer CPU invalidated pages in a shared address space.
            // vaddr == 0: full TLB flush (reload CR3).
            // vaddr != 0: single-page invlpg for that address.
            use core::sync::atomic::Ordering;
            let vaddr = super::apic::TLB_SHOOTDOWN_VADDR.load(Ordering::Acquire);
            if vaddr == 0 {
                // Full flush: reload CR3 to invalidate all user TLB entries.
                unsafe {
                    let cr3 = x86::controlregs::cr3();
                    x86::controlregs::cr3_write(cr3);
                }
            } else {
                unsafe {
                    core::arch::asm!("invlpg [{}]", in(reg) vaddr,
                        options(nostack, preserves_flags));
                }
            }
            crate::flight_recorder::record(
                crate::flight_recorder::kind::TLB_RECV,
                0,
                vaddr as u64,
                0,
            );
            let my_bit = 1u32 << super::cpu_id();
            super::apic::TLB_SHOOTDOWN_PENDING.fetch_and(!my_bit, Ordering::Release);
            ack_interrupt();
        }
        _ if vec >= VECTOR_IRQ_BASE => {
            ack_interrupt();

            let irq = vec - VECTOR_IRQ_BASE;
            match irq {
                TIMER_IRQ | TIMER_IRQ2 => { handler().handle_timer_irq(); }
                SERIAL0_IRQ => { super::serial::serial0_irq_handler(); }
                _ => { handler().handle_irq(irq); }
            }
            // Signal delivery is handled by x64_check_signal_on_irq_return
            // in trap.S after this function returns.
        }
        // Exceptions that should deliver a signal when caused by userspace.
        // CPL is stored in the low 2 bits of CS; non-zero means userspace.
        DIVIDE_ERROR_VECTOR
        | OVERFLOW_VECTOR
        | BOUND_RANGE_EXCEEDED_VECTOR
        | INVALID_OPCODE_VECTOR
        | GENERAL_PROTECTION_FAULT_VECTOR
        | STACK_SEGEMENT_FAULT_VECTOR
        | SEGMENT_NOT_PRESENT_VECTOR
        | X87_FPU_VECTOR
        | SIMD_FLOATING_POINT_VECTOR => {
            if frame.cs & 3 != 0 {
                let name = match vec {
                    DIVIDE_ERROR_VECTOR => "DIVIDE_ERROR",
                    OVERFLOW_VECTOR => "OVERFLOW",
                    BOUND_RANGE_EXCEEDED_VECTOR => "BOUND_RANGE_EXCEEDED",
                    INVALID_OPCODE_VECTOR => "INVALID_OPCODE",
                    GENERAL_PROTECTION_FAULT_VECTOR => "GENERAL_PROTECTION_FAULT",
                    STACK_SEGEMENT_FAULT_VECTOR => "STACK_SEGMENT_FAULT",
                    SEGMENT_NOT_PRESENT_VECTOR => "SEGMENT_NOT_PRESENT",
                    X87_FPU_VECTOR => "X87_FPU",
                    SIMD_FLOATING_POINT_VECTOR => "SIMD_FLOATING_POINT",
                    _ => unreachable!(),
                };
                handler().handle_user_fault(name, frame.rip as usize);
            } else {
                // Copy all packed fields to locals before use (packed struct UB).
                let rip    = frame.rip;
                let rsp    = frame.rsp;
                let rbp    = frame.rbp;
                let rax    = frame.rax;
                let rbx    = frame.rbx;
                let rcx    = frame.rcx;
                let rdx    = frame.rdx;
                let rsi    = frame.rsi;
                let rdi    = frame.rdi;
                let r8     = frame.r8;
                let r9     = frame.r9;
                let r10    = frame.r10;
                let r11    = frame.r11;
                let r12    = frame.r12;
                let r13    = frame.r13;
                let r14    = frame.r14;
                let r15    = frame.r15;
                let cs     = frame.cs;
                let rflags = frame.rflags;
                let ss     = frame.ss;
                let error  = frame.error;
                warn!("kernel exception {} — register dump:", vec);
                warn!("  RIP={:016x}  RSP={:016x}  RBP={:016x}", rip, rsp, rbp);
                warn!("  RAX={:016x}  RBX={:016x}  RCX={:016x}  RDX={:016x}", rax, rbx, rcx, rdx);
                warn!("  RSI={:016x}  RDI={:016x}  R8 ={:016x}  R9 ={:016x}", rsi, rdi, r8, r9);
                warn!("  R10={:016x}  R11={:016x}  R12={:016x}  R13={:016x}", r10, r11, r12, r13);
                warn!("  R14={:016x}  R15={:016x}", r14, r15);
                warn!("  CS={:#x} ({})  SS={:#x}  RFLAGS={:#010x}  ERR={:#x}",
                    cs,
                    if cs & 3 == 0 { "ring 0" } else { "ring 3" },
                    ss, rflags, error);
                crate::backtrace::print_interrupted_context(rip, rbp);
                panic!("kernel exception: vec={}, {:?}", vec, frame);
            }
        }
        DEBUG_VECTOR => {
            // TODO:
            panic!("unsupported exception: DEBUG\n{:?}", frame);
        }
        NONMASKABLE_INTERRUPT_VECTOR => {
            // TODO:
            panic!("unsupported exception: NONMASKABLE_INTERRUPT\n{:?}", frame);
        }
        BREAKPOINT_VECTOR => {
            // TODO:
            panic!("unsupported exception: BREAKPOINT\n{:?}", frame);
        }
        DEVICE_NOT_AVAILABLE_VECTOR => {
            // TODO:
            panic!("unsupported exception: DEVICE_NOT_AVAILABLE\n{:?}", frame);
        }
        DOUBLE_FAULT_VECTOR => {
            // TODO:
            panic!("unsupported exception: DOUBLE_FAULT\n{:?}", frame);
        }
        COPROCESSOR_SEGMENT_OVERRUN_VECTOR => {
            // TODO:
            panic!(
                "unsupported exception: COPROCESSOR_SEGMENT_OVERRUN\n{:?}",
                frame
            );
        }
        INVALID_TSS_VECTOR => {
            // TODO:
            panic!("unsupported exception: INVALID_TSS\n{:?}", frame);
        }
        PAGE_FAULT_VECTOR => {
            let reason = PageFaultReason::from_bits_truncate(frame.error as u32);

            // Hot path: user-mode page fault → demand paging.
            // Check the common case (CAUSED_BY_USER) first with a single branch.
            if reason.contains(PageFaultReason::CAUSED_BY_USER) {
                let unaligned_vaddr = UserVAddr::new(cr2());
                handler().handle_page_fault(unaligned_vaddr, frame.rip as usize, reason);
            } else {
                // Cold path: kernel fault or usercopy fault.
                handle_kernel_page_fault(frame, reason);
            }
        }
        ALIGNMENT_CHECK_VECTOR => {
            // TODO:
            panic!("unsupported exception: ALIGNMENT_CHECK\n{:?}", frame);
        }
        MACHINE_CHECK_VECTOR => {
            // TODO:
            panic!("unsupported exception: MACHINE_CHECK\n{:?}", frame);
        }
        VIRTUALIZATION_VECTOR => {
            // TODO:
            panic!("unsupported exception: VIRTUALIZATION\n{:?}", frame);
        }
        _ => {
            panic!("unexpected interrupt: vec={}", vec);
        }
    }
}

/// Cold path for kernel-mode page faults and usercopy faults.
/// Extracted from the hot interrupt dispatch to reduce icache pressure
/// in the user-mode page fault path (the common case for demand paging).
#[cold]
#[inline(never)]
fn handle_kernel_page_fault(frame: &InterruptFrame, reason: PageFaultReason) {
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn usercopy1();
        fn usercopy1b();
        fn usercopy1c();
        fn usercopy1d();
        fn usercopy2();
        fn usercopy3();
    }

    let occurred_in_usercopy = frame.rip == usercopy1 as *const u8 as u64
        || frame.rip == usercopy1b as *const u8 as u64
        || frame.rip == usercopy1c as *const u8 as u64
        || frame.rip == usercopy1d as *const u8 as u64
        || frame.rip == usercopy2 as *const u8 as u64
        || frame.rip == usercopy3 as *const u8 as u64;

    if occurred_in_usercopy {
        // Usercopy fault in kernel — handle as user page fault.
        let unaligned_vaddr = UserVAddr::new(unsafe { cr2() });
        handler().handle_page_fault(unaligned_vaddr, frame.rip as usize, reason);
        return;
    }

    // True kernel page fault — dump registers and panic.
    let rip    = frame.rip;
    let rsp    = frame.rsp;
    let rbp    = frame.rbp;
    let rax    = frame.rax;
    let rbx    = frame.rbx;
    let rcx    = frame.rcx;
    let rdx    = frame.rdx;
    let rsi    = frame.rsi;
    let rdi    = frame.rdi;
    let r8     = frame.r8;
    let r9     = frame.r9;
    let r10    = frame.r10;
    let r11    = frame.r11;
    let r12    = frame.r12;
    let r13    = frame.r13;
    let r14    = frame.r14;
    let r15    = frame.r15;
    let cs     = frame.cs;
    let rflags = frame.rflags;
    let ss     = frame.ss;
    let error  = frame.error;
    let vaddr  = unsafe { cr2() };
    warn!("kernel page fault — register dump:");
    warn!("  RIP={:016x}  RSP={:016x}  RBP={:016x}", rip, rsp, rbp);
    warn!("  RAX={:016x}  RBX={:016x}  RCX={:016x}  RDX={:016x}", rax, rbx, rcx, rdx);
    warn!("  RSI={:016x}  RDI={:016x}  R8 ={:016x}  R9 ={:016x}", rsi, rdi, r8, r9);
    warn!("  R10={:016x}  R11={:016x}  R12={:016x}  R13={:016x}", r10, r11, r12, r13);
    warn!("  R14={:016x}  R15={:016x}", r14, r15);
    warn!("  CS={:#x} ({})  SS={:#x}  RFLAGS={:#010x}  ERR={:#x}",
        cs,
        if cs & 3 == 0 { "ring 0" } else { "ring 3" },
        ss, rflags, error);
    warn!("  CR2 (fault vaddr) = {:016x}", vaddr);
    warn!("  kernel stack at RSP ({:016x}):", rsp);
    for i in 0..8usize {
        let addr = rsp as usize + i * 8;
        if VAddr::is_accessible_from_kernel(addr) {
            let val = unsafe { *(addr as *const u64) };
            warn!("    [rsp+{:#04x}] = {:016x}", i * 8, val);
        }
    }
    crate::backtrace::print_interrupted_context(rip, rbp);
    panic!(
        "page fault occurred in the kernel: rip={:x}, rsp={:x}, vaddr={:x}",
        rip, rsp, vaddr
    );
}

/// Called from trap.S after `x64_handle_interrupt` returns, with `frame`
/// pointing to the CURRENT thread's InterruptFrame (which may be a different
/// thread than the one that was running when the interrupt fired, due to a
/// context switch inside the interrupt handler).
///
/// Delivers any pending signal to the current thread before IRET returns it
/// to user space.  The trap.S caller already checked `frame.cs & 3 != 0`.
#[unsafe(no_mangle)]
unsafe extern "C" fn x64_check_signal_on_irq_return(frame: *mut InterruptFrame) {
    // Fast path: skip PtRegs construction if no signals are pending.
    // This avoids copying 20 register fields on every interrupt return
    // when no signal delivery is needed (the common case for page faults).
    let current = handler().current_process_signal_pending();
    if current == 0 {
        return;
    }

    use super::syscall::PtRegs;
    let frame = &mut *frame;
    let mut pt = PtRegs {
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        rbp: frame.rbp,
        rbx: frame.rbx,
        r11: frame.r11,
        r10: frame.r10,
        r9:  frame.r9,
        r8:  frame.r8,
        rax: frame.rax,
        rcx: frame.rcx,
        rdx: frame.rdx,
        rsi: frame.rsi,
        rdi: frame.rdi,
        orig_rax: 0,
        rip: frame.rip,
        cs:  frame.cs,
        rflags: frame.rflags,
        rsp: frame.rsp,
        ss:  frame.ss,
    };
    handler().handle_interrupt_return(&mut pt);
    // Write back fields that signal delivery may have modified.
    frame.rip    = pt.rip;
    frame.rsp    = pt.rsp;
    frame.rdi    = pt.rdi;
    frame.rsi    = pt.rsi;
    frame.rdx    = pt.rdx;
}

pub struct SavedInterruptStatus {
    rflags: RFlags,
}

impl SavedInterruptStatus {
    pub fn save() -> SavedInterruptStatus {
        SavedInterruptStatus {
            rflags: rflags::read(),
        }
    }
}

impl Drop for SavedInterruptStatus {
    fn drop(&mut self) {
        rflags::set(rflags::read() | (self.rflags & rflags::RFlags::FLAGS_IF));
    }
}
