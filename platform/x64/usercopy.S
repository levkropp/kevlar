/// void copy_from_user(void *dst, const void *src, size_t len);
/// void copy_to_user(void *dst, const void *src, size_t len);
///
/// Copies a memory buffer from/to the user space. We don't check the validity
/// of the user's addresses; instead, we handles a page fault occurred at
/// `usercopy` as a user's fault.
///
/// Note that caller MUST check that the memory range does not overlaps with the
/// kernel sapce!

// ── Usercopy trace ring buffer ──
//
// When ucopy_trace_on is non-zero, every copy_to_user / copy_from_user
// call records (dst, src, len, return_addr) into a fixed-size ring buffer.
// This is written in assembly so it captures the ACTUAL register values at
// entry -- not what the Rust caller thinks it passed.

.set UCOPY_TRACE_ENTRIES, 32
.set UCOPY_TRACE_ENTRY_SIZE, 32   // 4 x 8 bytes per entry

.text

.global copy_from_user, copy_to_user
.global usercopy1, usercopy1b, usercopy1c, usercopy1d
copy_from_user:
copy_to_user:
    // ── Trace probe (only when enabled) ──
    push rax
    lea rax, [rip + ucopy_trace_on]
    cmp qword ptr [rax], 0
    pop rax
    jne .Ltrace_entry
.Ltrace_done:

    cld
    // For copies >= 8 bytes, use qword bulk copy.
    cmp rdx, 8
    jb .Lbyte_copy

    // Copy leading bytes to align destination to 8-byte boundary.
    mov rcx, rdi
    neg rcx
    and rcx, 7
    jz .Laligned
    sub rdx, rcx
usercopy1:
    rep movsb

.Laligned:
    // Bulk copy as qwords.
    mov rcx, rdx
    shr rcx, 3
usercopy1b:
    rep movsq

    // Copy trailing bytes.
    mov rcx, rdx
    and rcx, 7
    jz .Ldone
usercopy1c:
    rep movsb
.Ldone:
    ret

.Lbyte_copy:
    mov rcx, rdx
usercopy1d:
    rep movsb
    ret

.Ltrace_entry:
    // Save registers we'll clobber for the trace write.
    // rdi, rsi, rdx must be preserved for the actual copy.
    push rax
    push rcx
    push r8
    push rdx              // save rdx (we use it as scratch below)

    // r8 = base of ucopy_trace_buf
    lea r8, [rip + ucopy_trace_buf]

    // rax = current index
    lea rcx, [rip + ucopy_trace_idx]
    mov rax, [rcx]

    // Compute entry offset: (idx & 31) * 32
    mov rcx, rax
    and rcx, (UCOPY_TRACE_ENTRIES - 1)
    imul rcx, rcx, UCOPY_TRACE_ENTRY_SIZE
    add r8, rcx

    pop rdx                // restore rdx before writing it

    // Write the 4-field entry: (dst=rdi, src=rsi, len=rdx, ret_addr)
    mov [r8 + 0],  rdi
    mov [r8 + 8],  rsi
    mov [r8 + 16], rdx
    // Return address is at [rsp + 24] (3 remaining pushes x 8 bytes)
    mov rcx, [rsp + 24]
    mov [r8 + 24], rcx

    // Advance index.
    inc rax
    lea rcx, [rip + ucopy_trace_idx]
    mov [rcx], rax

    pop r8
    pop rcx
    pop rax
    jmp .Ltrace_done

// ── Trace data ──
.data
.align 8
.global ucopy_trace_buf, ucopy_trace_idx, ucopy_trace_on
ucopy_trace_buf:
    .skip UCOPY_TRACE_ENTRIES * UCOPY_TRACE_ENTRY_SIZE
ucopy_trace_idx:
    .quad 0
ucopy_trace_on:
    .quad 0

.text

/// size_t strncpy_from_user(void *dst, const void *src, size_t max_len);
///
/// Copies NUL-terminated string from the userspace. It returns number of copied
/// characters. Unlike strcnpy, dst is NOT terminated by NULL.
.global strncpy_from_user, usercopy2
strncpy_from_user:
    mov rcx, rdx

    test rcx, rcx
    jz 1f

usercopy2:
    mov al, [rsi]

    test al, al
    jz 1f

    mov [rdi], al
    add rdi, 1
    add rsi, 1
    loop usercopy2

1:
    sub rdx, rcx
    mov rax, rdx
    ret

/// void memset_user(void *dst, uint8_t value, size_t len);
.global memset_user, usercopy3
memset_user:
    mov rcx, rdx
    cld
    mov al, sil
usercopy3:
    rep stosb
    ret
