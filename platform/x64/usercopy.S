/// void copy_from_user(void *dst, const void *src, size_t len);
/// void copy_to_user(void *dst, const void *src, size_t len);
///
/// Copies a memory buffer from/to the user space. We don't check the validity
/// of the user's addresses; instead, we handles a page fault occurred at
/// `usercopy` as a user's fault.
///
/// Note that caller MUST check that the memory range does not overlaps with the
/// kernel sapce!
.global copy_from_user, copy_to_user
.global usercopy1, usercopy1b, usercopy1c
copy_from_user:
copy_to_user:
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
    jmp usercopy1

/// size_t strncpy_from_user(void *dst, const void *src, size_t max_len);
///
/// Copies NUL-terminated string from the userspace. It returns number of copied
/// characters. Unlike strcnpy, `dst` is NOT terminated by NULL. I believe it
/// will never be a problem in Rust, by the way.
.global strncpy_from_user, usercopy2
strncpy_from_user:
    mov rcx, rdx

    test rcx, rcx
    jz 1f

usercopy2:
    // Read a character from the userspace.
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
///
/// Fills a memory buffer in the user space. We don't check the validity
/// of the user's addresses; instead, we handles a page fault occurred at
/// `usercopy` as a user's fault.
///
/// Note that caller MUST check that the memory range does not overlaps with the
/// kernel sapce!
.global memset_user, usercopy3
memset_user:
    mov rcx, rdx
    cld
usercopy3:
    mov al, sil
    rep stosb
    ret
