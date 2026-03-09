// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! vDSO (Virtual Dynamic Shared Object) for x86_64.
//!
//! Builds a single-page ELF shared object containing `__vdso_clock_gettime`.
//! This page is mapped read+execute into every user process, allowing
//! clock_gettime(CLOCK_MONOTONIC) to run entirely in userspace via rdtsc.
//!
//! musl's `__vdsosym()` parses the ELF to find the function pointer.  We
//! provide the minimum ELF metadata it needs: PT_DYNAMIC program header,
//! DT_HASH, DT_SYMTAB, DT_STRTAB, DT_STRSZ, DT_SYMENT.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::page_allocator::{alloc_pages, AllocPageFlags};
use crate::address::PAddr;
use crate::arch::PAGE_SIZE;

/// Physical address of the vDSO page (set once during init).
static VDSO_PADDR: AtomicU64 = AtomicU64::new(0);

/// Fixed virtual address where the vDSO is mapped in every process.
/// Placed above USER_VALLOC_END (0x0FFF_0000_0000) in unused user space.
/// PML4 index 32 — safely in user range (< 128).
pub const VDSO_VADDR: usize = 0x1000_0000_0000;

// ── Page layout ─────────────────────────────────────────────────────
// 0x000  ELF header          (64 bytes)
// 0x040  Program headers     (2 × 56 = 112 bytes)
// 0x0B0  Dynamic section     (6 × 16 = 96 bytes)
// 0x110  Symbol table        (2 × 24 = 48 bytes)
// 0x140  String table        (22 bytes)
// 0x160  SYSV hash table     (20 bytes)
// 0x200  Code                (~71 bytes)
// 0xF00  Data: tsc_origin    (8 bytes)
// 0xF08  Data: ns_mult       (8 bytes)

const OFF_EHDR: usize     = 0x000;
const OFF_PHDR: usize     = 0x040;
const OFF_DYNAMIC: usize  = 0x0B0;
const OFF_SYMTAB: usize   = 0x110;
const OFF_STRTAB: usize   = 0x140;
const OFF_HASH: usize     = 0x160;
const OFF_CODE: usize     = 0x200;
const OFF_DATA: usize     = 0xF00;

/// Offset of tsc_origin within the data area.
const OFF_TSC_ORIGIN: usize = OFF_DATA;
/// Offset of ns_mult within the data area.
const OFF_NS_MULT: usize    = OFF_DATA + 8;

/// Build the vDSO page and store its physical address.
/// Must be called after TSC calibration.
pub fn init() {
    let paddr = alloc_pages(1, AllocPageFlags::KERNEL)
        .expect("vdso: failed to allocate page");

    let base = paddr.as_vaddr().as_mut_ptr::<u8>();

    // Zero the page first.
    unsafe { base.write_bytes(0, PAGE_SIZE); }

    // Write all sections.
    unsafe {
        write_ehdr(base);
        write_phdrs(base);
        write_dynamic(base);
        write_symtab(base);
        write_strtab(base);
        write_hash(base);
        write_code(base);
        write_data(base);
    }

    VDSO_PADDR.store(paddr.value() as u64, Ordering::Release);
    info!("vdso: page at paddr {:#x}, mapped at vaddr {:#x}", paddr.value(), VDSO_VADDR);
}

/// Returns the physical address of the vDSO page, or None if not yet initialized.
pub fn page_paddr() -> Option<PAddr> {
    let v = VDSO_PADDR.load(Ordering::Acquire);
    if v == 0 { None } else { Some(PAddr::new(v as usize)) }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Write a little-endian u16 at the given offset.
unsafe fn w16(base: *mut u8, off: usize, val: u16) {
    let bytes = val.to_le_bytes();
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(off), 2);
}

/// Write a little-endian u32 at the given offset.
unsafe fn w32(base: *mut u8, off: usize, val: u32) {
    let bytes = val.to_le_bytes();
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(off), 4);
}

/// Write a little-endian u64 at the given offset.
unsafe fn w64(base: *mut u8, off: usize, val: u64) {
    let bytes = val.to_le_bytes();
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(off), 8);
}

/// Write a byte slice at the given offset.
unsafe fn wbytes(base: *mut u8, off: usize, data: &[u8]) {
    core::ptr::copy_nonoverlapping(data.as_ptr(), base.add(off), data.len());
}

// ── ELF Header (Elf64_Ehdr, 64 bytes) ──────────────────────────────

unsafe fn write_ehdr(base: *mut u8) {
    // e_ident
    wbytes(base, OFF_EHDR, &[
        0x7f, b'E', b'L', b'F',  // magic
        2,                         // EI_CLASS: ELFCLASS64
        1,                         // EI_DATA: ELFDATA2LSB
        1,                         // EI_VERSION: EV_CURRENT
        0,                         // EI_OSABI: ELFOSABI_NONE
        0, 0, 0, 0, 0, 0, 0, 0,   // padding
    ]);
    w16(base, OFF_EHDR + 16, 3);       // e_type: ET_DYN
    w16(base, OFF_EHDR + 18, 0x3e);    // e_machine: EM_X86_64
    w32(base, OFF_EHDR + 20, 1);       // e_version: EV_CURRENT
    w64(base, OFF_EHDR + 24, OFF_CODE as u64); // e_entry
    w64(base, OFF_EHDR + 32, OFF_PHDR as u64); // e_phoff
    w64(base, OFF_EHDR + 40, 0);       // e_shoff (no section headers)
    w32(base, OFF_EHDR + 48, 0);       // e_flags
    w16(base, OFF_EHDR + 52, 64);      // e_ehsize
    w16(base, OFF_EHDR + 54, 56);      // e_phentsize
    w16(base, OFF_EHDR + 56, 2);       // e_phnum
    w16(base, OFF_EHDR + 58, 0);       // e_shentsize
    w16(base, OFF_EHDR + 60, 0);       // e_shnum
    w16(base, OFF_EHDR + 62, 0);       // e_shstrndx
}

// ── Program Headers (2 × Elf64_Phdr, each 56 bytes) ────────────────

unsafe fn write_phdrs(base: *mut u8) {
    // PHDR 0: PT_LOAD — map entire page R+X
    let p0 = OFF_PHDR;
    w32(base, p0,      1);                    // p_type: PT_LOAD
    w32(base, p0 + 4,  5);                    // p_flags: PF_R | PF_X
    w64(base, p0 + 8,  0);                    // p_offset
    w64(base, p0 + 16, 0);                    // p_vaddr (relative to load base)
    w64(base, p0 + 24, 0);                    // p_paddr
    w64(base, p0 + 32, PAGE_SIZE as u64);     // p_filesz
    w64(base, p0 + 40, PAGE_SIZE as u64);     // p_memsz
    w64(base, p0 + 48, PAGE_SIZE as u64);     // p_align

    // PHDR 1: PT_DYNAMIC — points to the dynamic section
    let dynamic_size = 6 * 16; // 6 entries × 16 bytes each
    let p1 = OFF_PHDR + 56;
    w32(base, p1,      2);                    // p_type: PT_DYNAMIC
    w32(base, p1 + 4,  4);                    // p_flags: PF_R
    w64(base, p1 + 8,  OFF_DYNAMIC as u64);   // p_offset
    w64(base, p1 + 16, OFF_DYNAMIC as u64);   // p_vaddr
    w64(base, p1 + 24, OFF_DYNAMIC as u64);   // p_paddr
    w64(base, p1 + 32, dynamic_size as u64);  // p_filesz
    w64(base, p1 + 40, dynamic_size as u64);  // p_memsz
    w64(base, p1 + 48, 8);                    // p_align
}

// ── Dynamic Section (6 × Elf64_Dyn, each 16 bytes) ─────────────────

unsafe fn write_dynamic(base: *mut u8) {
    let d = OFF_DYNAMIC;
    // DT_HASH (4) → hash table
    w64(base, d,      4);
    w64(base, d + 8,  OFF_HASH as u64);
    // DT_STRTAB (5) → string table
    w64(base, d + 16, 5);
    w64(base, d + 24, OFF_STRTAB as u64);
    // DT_SYMTAB (6) → symbol table
    w64(base, d + 32, 6);
    w64(base, d + 40, OFF_SYMTAB as u64);
    // DT_STRSZ (10) → string table size
    w64(base, d + 48, 10);
    w64(base, d + 56, STRTAB.len() as u64);
    // DT_SYMENT (11) → sizeof(Elf64_Sym)
    w64(base, d + 64, 11);
    w64(base, d + 72, 24);
    // DT_NULL (0) → terminator
    w64(base, d + 80, 0);
    w64(base, d + 88, 0);
}

// ── Symbol Table (2 × Elf64_Sym, each 24 bytes) ────────────────────

unsafe fn write_symtab(base: *mut u8) {
    let s = OFF_SYMTAB;
    // Symbol 0: null (required by ELF spec) — already zeroed.

    // Symbol 1: __vdso_clock_gettime
    let s1 = s + 24;
    w32(base, s1,      1);           // st_name: offset 1 in strtab
    *base.add(s1 + 4) = 0x12;       // st_info: STB_GLOBAL | STT_FUNC
    *base.add(s1 + 5) = 0;          // st_other: STV_DEFAULT
    w16(base, s1 + 6,  1);          // st_shndx: non-zero (not SHN_UNDEF)
    w64(base, s1 + 8,  OFF_CODE as u64); // st_value: offset from load base
    w64(base, s1 + 16, 0x47);       // st_size: 71 bytes of code
}

// ── String Table ────────────────────────────────────────────────────

const STRTAB: &[u8] = b"\0__vdso_clock_gettime\0";

unsafe fn write_strtab(base: *mut u8) {
    wbytes(base, OFF_STRTAB, STRTAB);
}

// ── SYSV Hash Table ────────────────────────────────────────────────
//
// With nbucket=1 all symbols hash to bucket 0.  musl iterates all
// buckets and follows chains, so this always finds our symbol.
//
// Layout: [nbucket=1, nchain=2, bucket[0]=1, chain[0]=0, chain[1]=0]

unsafe fn write_hash(base: *mut u8) {
    let h = OFF_HASH;
    w32(base, h,      1);   // nbucket
    w32(base, h + 4,  2);   // nchain (number of symbols)
    w32(base, h + 8,  1);   // bucket[0] → symbol index 1
    w32(base, h + 12, 0);   // chain[0] → end (null symbol)
    w32(base, h + 16, 0);   // chain[1] → end (__vdso_clock_gettime)
}

// ── Code: __vdso_clock_gettime ──────────────────────────────────────
//
// int __vdso_clock_gettime(clockid_t clock_id, struct timespec *tp)
//
// Handles CLOCK_MONOTONIC (1) via rdtsc + fixed-point multiply.
// Returns -ENOSYS for other clock IDs (musl falls back to syscall).
//
// 0x200: cmp edi, 1
// 0x203: jne .fallback
// 0x209: rdtsc                         ; EDX:EAX = TSC
// 0x20b: shl rdx, 32
// 0x20f: or  rax, rdx                  ; RAX = 64-bit TSC
// 0x212: sub rax, [rip+0xce7]          ; delta = tsc - origin  (→ 0xF00)
// 0x219: mov rcx, [rip+0xce8]          ; mult = ns_mult        (→ 0xF08)
// 0x220: mul rcx                       ; RDX:RAX = delta * mult
// 0x223: shrd rax, rdx, 32             ; RAX = (delta*mult)>>32 = nanoseconds
// 0x228: mov rcx, 1000000000
// 0x232: xor edx, edx
// 0x234: div rcx                       ; RAX = seconds, RDX = nanoseconds
// 0x237: mov [rsi], rax
// 0x23a: mov [rsi+8], rdx
// 0x23e: xor eax, eax                  ; return 0
// 0x240: ret
// 0x241: mov eax, -38                  ; -ENOSYS
// 0x246: ret

#[rustfmt::skip]
const VDSO_CODE: [u8; 71] = [
    // cmp edi, 1
    0x83, 0xff, 0x01,
    // jne .fallback (rel32 = 0x38)
    0x0f, 0x85, 0x38, 0x00, 0x00, 0x00,
    // rdtsc
    0x0f, 0x31,
    // shl rdx, 32
    0x48, 0xc1, 0xe2, 0x20,
    // or rax, rdx
    0x48, 0x09, 0xd0,
    // sub rax, [rip+0xce7]  → tsc_origin at 0xF00
    0x48, 0x2b, 0x05, 0xe7, 0x0c, 0x00, 0x00,
    // mov rcx, [rip+0xce8]  → ns_mult at 0xF08
    0x48, 0x8b, 0x0d, 0xe8, 0x0c, 0x00, 0x00,
    // mul rcx
    0x48, 0xf7, 0xe1,
    // shrd rax, rdx, 32
    0x48, 0x0f, 0xac, 0xd0, 0x20,
    // mov rcx, 1000000000
    0x48, 0xb9, 0x00, 0xca, 0x9a, 0x3b, 0x00, 0x00, 0x00, 0x00,
    // xor edx, edx
    0x31, 0xd2,
    // div rcx
    0x48, 0xf7, 0xf1,
    // mov [rsi], rax
    0x48, 0x89, 0x06,
    // mov [rsi+8], rdx
    0x48, 0x89, 0x56, 0x08,
    // xor eax, eax
    0x31, 0xc0,
    // ret
    0xc3,
    // .fallback: mov eax, -38 (-ENOSYS)
    0xb8, 0xda, 0xff, 0xff, 0xff,
    // ret
    0xc3,
];

unsafe fn write_code(base: *mut u8) {
    wbytes(base, OFF_CODE, &VDSO_CODE);
}

// ── Data area (TSC parameters, written once at init) ────────────────

unsafe fn write_data(base: *mut u8) {
    let origin = super::tsc::tsc_origin();
    let mult = super::tsc::ns_mult();
    w64(base, OFF_TSC_ORIGIN, origin);
    w64(base, OFF_NS_MULT, mult);
}
