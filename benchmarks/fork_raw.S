/* Minimal fork+exit benchmark using raw syscall instructions.
   No libc, no stack frames, no SSE — just syscall/sysret. */
.global _start
_start:
    /* Mount /proc for clock_gettime */
    mov $165, %rax          /* SYS_mount */
    lea proc_str(%rip), %rdi
    lea proc_path(%rip), %rsi
    lea proc_str(%rip), %rdx
    xor %r10, %r10
    xor %r8, %r8
    syscall

    /* Warm up: 5 fork+exit iterations */
    mov $5, %r15
warmup:
    mov $57, %rax           /* SYS_fork */
    syscall
    test %rax, %rax
    jz child_exit
    mov %rax, %rdi
    mov $61, %rax           /* SYS_wait4 */
    xor %rsi, %rsi
    xor %rdx, %rdx
    xor %r10, %r10
    syscall
    dec %r15
    jnz warmup

    /* Read start TSC */
    rdtsc
    shl $32, %rdx
    or %rdx, %rax
    mov %rax, %r14          /* r14 = start TSC */

    /* Benchmark: 200 iterations */
    mov $200, %r15
bench_loop:
    mov $57, %rax           /* SYS_fork */
    syscall
    test %rax, %rax
    jz child_exit
    /* Parent: waitpid(child, NULL, 0) */
    mov %rax, %rdi
    mov $61, %rax           /* SYS_wait4 */
    xor %rsi, %rsi
    xor %rdx, %rdx
    xor %r10, %r10
    syscall
    dec %r15
    jnz bench_loop

    /* Read end TSC */
    rdtsc
    shl $32, %rdx
    or %rdx, %rax
    sub %r14, %rax          /* rax = elapsed cycles */

    /* Print result: divide by 200, convert to ns at ~2400 MHz */
    /* total_ns = cycles * 1000 / 2400 */
    /* per_iter_ns = total_ns / 200 */
    mov %rax, %rdi
    mov $1000, %rsi
    imul %rsi
    mov $2400, %rsi
    xor %rdx, %rdx
    div %rsi                /* rax = total_ns */
    mov %rax, %r13          /* save total_ns */
    mov $200, %rsi
    xor %rdx, %rdx
    div %rsi                /* rax = per_iter_ns */

    /* Format: "BENCH fork_raw 200 <total> <per_iter>\n" */
    /* Simple: just write the per-iter value */
    push %rax
    lea msg(%rip), %rsi
    mov $msg_len, %rdx
    mov $1, %rdi
    mov $1, %rax            /* SYS_write */
    syscall

    /* Write per_iter number */
    pop %rax
    call print_num

    /* Newline */
    lea nl(%rip), %rsi
    mov $1, %rdx
    mov $1, %rdi
    mov $1, %rax
    syscall

    /* exit(0) */
    xor %rdi, %rdi
    mov $231, %rax          /* SYS_exit_group */
    syscall

child_exit:
    xor %rdi, %rdi
    mov $231, %rax          /* SYS_exit_group */
    syscall

/* Print rax as decimal */
print_num:
    mov %rsp, %r12
    sub $32, %rsp
    mov %rsp, %rsi
    add $31, %rsi
    movb $0, (%rsi)
    mov $10, %rcx
.Ldigit:
    dec %rsi
    xor %rdx, %rdx
    div %rcx
    add $'0', %dl
    mov %dl, (%rsi)
    test %rax, %rax
    jnz .Ldigit
    /* rsi points to first digit */
    mov %r12, %rax
    sub %rsi, %rax          /* length */
    mov %rax, %rdx
    mov $1, %rdi
    mov $1, %rax
    syscall
    mov %r12, %rsp
    ret

.section .rodata
proc_str: .asciz "proc"
proc_path: .asciz "/proc"
msg: .ascii "fork_raw ns/iter: "
.set msg_len, . - msg
nl: .ascii "\n"
