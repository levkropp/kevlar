//
//  Interrupt/exception handlers
//
.set INTERRUPT_HANDLER_SIZE, 16

// Must match the struct in cpu_local.rs (static_assert at bottom of that
// file enforces it).
.set GS_QSC_COUNTER, 24

.align INTERRUPT_HANDLER_SIZE
.global interrupt_handlers
interrupt_handlers:
.set i, 0
.rept 256
.set handler_start, .
// Exceptions with error code.
.if i == 8 || 10 <= i && i <= 14 || i == 17
    .align INTERRUPT_HANDLER_SIZE
    cli
    push i
    jmp interrupt_common
    .align INTERRUPT_HANDLER_SIZE
// Interrupts and exceptions without error code.
.else
    .align INTERRUPT_HANDLER_SIZE
    cli
    push 0 // Dummy value as error code.
    push i
    jmp interrupt_common
    .align INTERRUPT_HANDLER_SIZE
.endif

// Increment the counter.
.set i, i + 1
.endr

.extern x64_handle_interrupt
interrupt_common:
    //
    //  The current stack frame:
    //
    //            +--------------------+
    //     48     |        SS          |
    //            +--------------------+
    //     40     |        RSP         |
    //            +--------------------+
    //     32     |       RFLAGS       |
    //            +--------------------+
    //     24     |        CS          |
    //            +--------------------+
    //     16     |        RIP         |
    //            +--------------------+
    //      8     |     Error code     |
    //            +--------------------+
    //      0     |     IRQ Number     | <- RSP
    //            +--------------------+
    //

    // Check CS register in the IRET frame to determine if the interrupt has
    // occurred in user mode.
    test qword ptr [rsp + 24], 3
    jz 1f
    swapgs
1:
    // Clear DF unconditionally: a kernel-mode interrupt can fire while a
    // kernel function is mid-way through a `rep movsb`/`rep stosb` (DF=0)
    // or a userspace function set DF=1 before being interrupted. The kernel
    // ABI requires DF=0 in handler bodies, and our memcpy/memset rely on it.
    cld

    // Save RDI and set the IRQ number to RDI at once.
    xchg rdi, [rsp]

    // Save registers except RDI (we have already saved it above).
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rbp
    push rsi
    push rdx
    push rcx
    push rbx
    push rax

    mov rsi, rsp
    call x64_handle_interrupt

    // After x64_handle_interrupt (which may have context-switched via
    // do_switch_thread), RSP points to the CURRENT thread's InterruptFrame.
    // This is correct even after a switch: do_switch_thread changed RSP to
    // the next thread's kernel stack, so the frame here belongs to the thread
    // we are about to IRET into.
    //
    // Deliver pending signals to the current thread now, before IRET, if
    // returning to user space.  CS is at offset 136 in InterruptFrame.
    //
    // Alignment note: at this point RSP ≡ 8 (mod 16) for ring-3 interrupts
    // (kernel stack top is 16-byte aligned; ring-3 entry pushes 21 qwords).
    // sub rsp,8 makes RSP ≡ 0 (mod 16) satisfying the ABI before CALL.
    test qword ptr [rsp + 136], 3
    jz 1f
    lea rdi, [rsp]
    sub rsp, 8
    call x64_check_signal_on_irq_return
    add rsp, 8
1:
    // QSC: interrupt is about to return.  IF is still 0 here (set by the
    // interrupt stub) so no nested IRQ can double-count.  Any hardware
    // walker that started before this interrupt has retired: the
    // interrupt vectored the CPU into kernel mode, which serializes
    // with prior walks.  See Vm::Drop for the grace-period consumer.
    inc qword ptr gs:[GS_QSC_COUNTER]

    pop rax
    pop rbx
    pop rcx
    pop rdx
    pop rsi
    pop rbp
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
    pop rdi

    // Skip error code.
    add rsp, 8

    // Check CS register in the IRET frame to determine whether the exception
    // occur in the userspace. If so, do SWAPGS and force IF=1.
    test qword ptr [rsp + 8], 3
    jz 1f

    // Force IF=1 in saved RFLAGS: userspace must not leave interrupts disabled.
    // Xorg with iopl(3) can execute cli, clearing IF. Without this fix,
    // IRET restores IF=0 and the PIT timer stops on this CPU.
    or qword ptr [rsp + 16], 0x200

    cli
    swapgs
1:
    iretq
