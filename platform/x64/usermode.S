// Offsets in CpuLocalHead.  Must match the struct in cpu_local.rs
// (static_assert at bottom of that file enforces it).
.set GS_RSP0,          0
.set GS_RSP3,          8
.set GS_PREEMPT_COUNT, 16
.set GS_NEED_RESCHED,  20
.set GS_QSC_COUNTER,   24

.global syscall_entry
syscall_entry:
    // "A.2.1 Calling Conventions" in https://uclibc.org/docs/psABI-x86_64.pdf
    //
    //   RAX: syscall number
    //   RDI: arg1
    //   RSI: arg2
    //   RDX: arg3
    //   RCX: user RIP (set by SYSCALL)
    //   R8:  arg5
    //   R9:  arg6
    //   R10: arg4
    //   R11: user RFLAGS (set by SYSCALL)
    //   The others: user values (need to be preserved)

    // Interrupts are automatically disabled by MSR_FMASK. If interrupts are
    // enabled, an interrupt may occurs before SWAPGS and as a result, an
    // interrupt handler mistakenly use the user's GS base because the interrupt
    // has occurred in the kernel mode and SWAPGS won't be performed.
    swapgs

    // Clear DF: x86_64 SysV ABI requires DF=0 across function calls, but
    // user code is allowed to set it (e.g., glibc memmove for backward
    // copy uses std + rep movsb + cld; a signal interrupting between std
    // and cld would enter the kernel with DF=1). Without cld here, kernel
    // `rep stosb`/`rep movsb` (used by memset/memcpy) would run backward,
    // corrupting memory BELOW the intended target — including saved RIP
    // slots on the kernel stack. Linux does the same.
    cld

    // Switch to the kernel stack.
    mov gs:[GS_RSP3], rsp
    mov rsp, gs:[GS_RSP0]

    // QSC: entering kernel via SYSCALL is a quiescent point.  Any prior
    // user-space hardware page walker has retired before the CPU
    // transitioned into kernel mode.  Vm::Drop reads every CPU's
    // qsc_counter to implement a grace period before freeing PT pages.
    inc qword ptr gs:[GS_QSC_COUNTER]

    // Save SYSRET context and registers (pt_regs) onto the kernel stack.
    push 32           // User SS (USER_DS). FIXME: Hardcoded.
    push gs:[GS_RSP3] // User RSP.
    push r11          // User RFLAGS.
    push 40           // User CS (USER_CS64). FIXME: Hardcoded.
    push rcx          // User RIP.
    push rax          // orig_rax
    push rdi
    push rsi
    push rdx
    push rcx
    push rax          // User RAX.
    push r8
    push r9
    push r10
    push r11
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15

    mov rcx, r10 // arg3
    push rsp     // syscall frame
    push rax     // syscall number
    // Re-enable interrupts for the syscall body.  SYSCALL masks IF=0 via
    // IA32_FMASK; leaving IF=0 silently degrades flush_tlb_remote to a
    // PCID-generation bump only (blog 188), producing kernel-pointer
    // leaks via stale TLB entries.  Safe now because the per-CPU QSC
    // grace period (commit 253821f) serializes Vm::Drop against
    // in-flight hardware page walkers.
    sti
    call x64_handle_syscall
    cli
    add rsp, 16

    // Restore registers.
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    add rsp, 8 // User R11
    pop r10
    pop r9
    pop r8
    add rsp, 16 // User RAX & RCX
    pop rdx
    pop rsi
    pop rdi
    add rsp, 8 // orig_rax
    pop rcx    // User RIP
    add rsp, 8 // User CS
    pop r11    // User RFLAGS
    or r11, 0x200  // Force IF=1: userspace must not leave interrupts disabled.
                   // Xorg with iopl(3) can execute cli, which clears IF.
                   // Without this, SYSRET restores IF=0 and the PIT timer
                   // stops on this CPU, breaking all sleep/nanosleep timers.
    pop rsp

    // Ensure that interrupts are disabled since SWAPGS and SYSRETQ are not a
    // atomic operation. If an interrupt occurred between SWAPGS and SYSRETQ
    // (thus the GS base points to the user one), the interrupt handler won't
    // perform SWAPGS since the interrupt has occurred in kernel mode.
    cli
    swapgs
    sysretq

.global kthread_entry
kthread_entry:
    sti

    // Clear the frame pointer to stop backtracing here.
    xor rbp, rbp

    // Pop the entry point and start executing.
    pop rax
    call rax

    // Unreachable.
    ud2

.global userland_entry
userland_entry:
    // preempt_count is now per-thread and restored by do_switch_thread
    // (pop rax; mov gs:[GS_PREEMPT_COUNT], eax) from the initial stack
    // layout, which stores 0 for a fresh thread.  The old hand-rolled
    // `dec gs:[GS_PREEMPT_COUNT]` was needed when preempt_count was
    // per-CPU — switch()'s preempt_disable++ would otherwise leak onto
    // the new thread's first instructions — but would underflow to
    // 0xFFFFFFFF on a freshly-created thread whose initial stored
    // preempt_count is 0.

    // Sanitize registers to prevent information leak.
    xor rax, rax
    xor rbx, rbx
    xor rcx, rcx
    xor rdx, rdx
    xor rdi, rdi
    xor rsi, rsi
    xor r8, r8
    xor r9, r9
    xor r10, r10
    xor r11, r11
    xor r12, r12
    xor r13, r13
    xor r14, r14
    xor r15, r15
    xor rbp, rbp

    cli
    swapgs
    iretq

.global forked_child_entry
forked_child_entry:
    // preempt_count is now per-thread and restored by do_switch_thread
    // from the fork()-constructed stack, which stores 0 for the child.
    // See userland_entry above for why the hand-rolled dec was removed.

    pop rdx
    pop rdi
    pop rsi
    pop r8
    pop r9
    pop r10

    // Restore SYSCALL-related registers.
    pop rcx
    pop r11

    // Set fork(2)'s the return value.
    xor rax, rax

    cli
    swapgs
    iretq

/// fn do_switch_thread(prev_rsp: *mut u64, next_rsp: *const u64, ctx_saved: *mut u8);
///
/// Saves the current context into `prev` and restores the context from `next`.
///
/// After saving prev's RSP, sets *ctx_saved = 1 (release store; on x86 all
/// stores are TSO-release so a plain byte store suffices).  This lets
/// resume() on another CPU stop spinning before it enqueues this thread.
///
/// A *context* is a state of registers including:
///
///   - Instruction Pointer (implicitly saved and restored by CALL/RET)
///   - Stack Pointer (RSP)
///   - callee-saved registers (RBP, RBX, R12-R15)
///   - RFLAGS
///
.global do_switch_thread
do_switch_thread:
    /* Save RFLAGS first (deepest on the saved frame). */
    pushfq

    /* Save preempt_count as part of the thread's context.  Without
       this, CpuLocalHead.preempt_count would "stick" across context
       switches — prev yielding with a nonzero depth (e.g. holding a
       lock_no_irq that bumped preempt_count) would leak that depth
       onto next's execution.  Per-thread preempt_count matches
       Linux's thread_info->preempt_count and is a precondition for
       broad sti in syscall_entry.
       Pushed AFTER rflags so the restore can pop preempt_count while
       IF is still 0 (before popfq) — avoids a window where IF is
       re-enabled but the per-thread count hasn't been set yet. */
    xor rax, rax
    mov eax, dword ptr gs:[GS_PREEMPT_COUNT]
    push rax

    /* Save callee-saved registers.
       IMPORTANT: push order must match task.rs's initial stack layouts
       (RIP, RFLAGS, preempt_count, R15-R12, RBX, RBP — popped in reverse). */
    push r15
    push r14
    push r13
    push r12
    push rbx
    push rbp

    /* Save prev's RSP, then mark its context as saved (rdx = ctx_saved ptr). */
    mov [rdi], rsp
    mov byte ptr [rdx], 1

    /* Disable interrupts before loading the new stack to prevent a timer
       IRQ from pushing a frame during the restore sequence. */
    cli

    /* Load next's RSP. */
    mov rsp, [rsi]

    /* (canary check removed — done in Rust instead) */
    /* Restore callee-saved registers (reverse of push order). */
    pop rbp
    pop rbx
    pop r12
    pop r13
    pop r14
    pop r15

    /* Restore preempt_count BEFORE popfq (while IF still 0), so no IRQ
       handler observes a mismatched preempt_count/need_resched state. */
    pop rax
    mov dword ptr gs:[GS_PREEMPT_COUNT], eax

    /* QSC: the CPU just finished loading a fresh thread's context; any
       page walker from the outgoing thread is retired (its stack is
       parked on a runqueue).  IF still 0 here so no nested IRQ can
       double-count this. */
    inc qword ptr gs:[GS_QSC_COUNTER]

    popfq       /* Restores IF — re-enables interrupts if they were on. */

    /* Resume the next thread. */
    ret
