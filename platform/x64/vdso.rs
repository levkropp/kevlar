// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! vDSO (Virtual Dynamic Shared Object) for x86_64.
//!
//! Builds a single-page ELF shared object containing 7 vDSO functions:
//!   - `__vdso_clock_gettime`  — rdtsc + fixed-point → struct timespec
//!   - `__vdso_gettimeofday`   — rdtsc + fixed-point → struct timeval
//!   - `__vdso_getpid`         — read from data page
//!   - `__vdso_gettid`         — read from data page (0 → syscall fallback)
//!   - `__vdso_getuid`         — read from data page
//!   - `__vdso_getpriority`    — 20 - nice from data page
//!   - `__vdso_uname`          — memcpy 390 bytes from data page
//!
//! Each process gets its own physical vDSO page with per-process data
//! (pid, tid, uid, nice, utsname).  The template page (built at boot by
//! `init()`) contains the ELF metadata, code, and TSC calibration data.
//! `alloc_process_page()` clones the template and writes per-process fields.
//!
//! musl only looks up `__vdso_clock_gettime` and `__vdso_gettimeofday`.
//! The identity syscall symbols are infrastructure for glibc (M10+).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::page_allocator::{alloc_pages, AllocPageFlags};
use crate::address::PAddr;
use crate::arch::PAGE_SIZE;

/// Physical address of the template vDSO page (set once during init).
static VDSO_PADDR: AtomicU64 = AtomicU64::new(0);

/// Fixed virtual address where the vDSO is mapped in every process.
/// Placed above USER_VALLOC_END (0x0FFF_0000_0000) in unused user space.
/// PML4 index 32 — safely in user range (< 128).
pub const VDSO_VADDR: usize = 0x1000_0000_0000;

// ── Page layout ─────────────────────────────────────────────────────
// 0x000  ELF header          (64 bytes)
// 0x040  Program headers     (2 × 56 = 112 bytes)
// 0x0B0  Dynamic section     (6 × 16 = 96 bytes)
// 0x110  Symbol table        (8 × 24 = 192 bytes)
// 0x1D0  String table        (116 bytes)
// 0x248  SYSV hash table     (44 bytes)
// 0x300  Code                (~212 bytes)
//        0x300  __vdso_clock_gettime  (84 bytes) — REALTIME + MONOTONIC
//        0x354  __vdso_gettimeofday   (8 bytes)  — syscall fallback
//        0x35C  __vdso_getpid         (7 bytes)
//        0x363  __vdso_gettid         (19 bytes)
//        0x376  __vdso_getuid         (7 bytes)
//        0x37D  __vdso_getpriority    (28 bytes)
//        0x399  __vdso_uname          (17 bytes)
// 0xE00  Data area
//        0xE00  tsc_origin       (u64)
//        0xE08  ns_mult          (u64)
//        0xE10  wall_epoch_ns    (u64)  — RTC boot epoch in nanoseconds
//        0xE18  pid              (i32)
//        0xE1C  tid              (i32)
//        0xE20  uid              (u32)
//        0xE24  nice             (i32)
//        0xE28  utsname          (390 bytes, ends at 0xFAE)

const OFF_EHDR: usize     = 0x000;
const OFF_PHDR: usize     = 0x040;
const OFF_DYNAMIC: usize  = 0x0B0;
const OFF_SYMTAB: usize   = 0x110;
const OFF_STRTAB: usize   = 0x1D0;
const OFF_HASH: usize     = 0x248;
const OFF_CODE: usize     = 0x300;
const OFF_DATA: usize     = 0xE00;

// Data field offsets.
const OFF_TSC_ORIGIN: usize    = OFF_DATA;        // 0xE00
const OFF_NS_MULT: usize      = OFF_DATA + 8;     // 0xE08
const OFF_WALL_EPOCH_NS: usize = OFF_DATA + 0x10; // 0xE10 — RTC boot epoch in nanoseconds
const OFF_PID: usize           = OFF_DATA + 0x18;  // 0xE18
const OFF_TID: usize           = OFF_DATA + 0x1C;  // 0xE1C
const OFF_UID: usize           = OFF_DATA + 0x20;  // 0xE20
const OFF_NICE: usize          = OFF_DATA + 0x24;  // 0xE24
const OFF_UTSNAME: usize       = OFF_DATA + 0x28;  // 0xE28

// Code sub-offsets (absolute page offsets for each function).
const OFF_CLOCK_GETTIME: usize  = 0x300;  // 84 bytes
const OFF_GETTIMEOFDAY: usize   = 0x354;  // 8 bytes (syscall fallback)
const OFF_GETPID: usize         = 0x35C;  // 7 bytes
const OFF_GETTID: usize         = 0x363;  // 19 bytes
const OFF_GETUID: usize         = 0x376;  // 7 bytes
const OFF_GETPRIORITY: usize    = 0x37D;  // 28 bytes
const OFF_UNAME: usize          = 0x399;  // 17 bytes

/// Build the template vDSO page and store its physical address.
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
    info!("vdso: template page at paddr {:#x}, mapped at vaddr {:#x}", paddr.value(), VDSO_VADDR);
}

/// Returns the physical address of the template vDSO page, or None if not yet initialized.
pub fn page_paddr() -> Option<PAddr> {
    let v = VDSO_PADDR.load(Ordering::Acquire);
    if v == 0 { None } else { Some(PAddr::new(v as usize)) }
}

/// Allocate a per-process vDSO page by cloning the template and writing
/// per-process data (pid, tid, uid, nice, utsname).
pub fn alloc_process_page(pid: i32, tid: i32, uid: u32, nice: i32, utsname: &[u8; 390]) -> Option<PAddr> {
    let template_paddr = page_paddr()?;
    let new_paddr = alloc_pages(1, AllocPageFlags::KERNEL)
        .expect("vdso: failed to allocate process page");

    unsafe {
        let src = template_paddr.as_vaddr().as_ptr::<u8>();
        let dst = new_paddr.as_vaddr().as_mut_ptr::<u8>();
        core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);

        // Write per-process data fields.
        w32(dst, OFF_PID, pid as u32);
        w32(dst, OFF_TID, tid as u32);
        w32(dst, OFF_UID, uid);
        w32(dst, OFF_NICE, nice as u32);
        wbytes(dst, OFF_UTSNAME, utsname);
    }

    Some(new_paddr)
}

/// Update the TID field in a per-process vDSO page.
/// Called when creating threads (set tid=0 so __vdso_gettid falls back to syscall).
pub fn update_tid(paddr: PAddr, tid: i32) {
    unsafe {
        w32(paddr.as_vaddr().as_mut_ptr::<u8>(), OFF_TID, tid as u32);
    }
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
    w64(base, OFF_EHDR + 24, OFF_CODE as u64); // e_entry: __vdso_clock_gettime
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

// ── Symbol Table (8 × Elf64_Sym, each 24 bytes) ────────────────────

/// Write one Elf64_Sym entry.
unsafe fn write_sym(base: *mut u8, index: usize, name_off: u32, value: u64, size: u64) {
    let s = OFF_SYMTAB + index * 24;
    w32(base, s,      name_off);    // st_name
    *base.add(s + 4) = 0x12;       // st_info: STB_GLOBAL | STT_FUNC
    *base.add(s + 5) = 0;          // st_other: STV_DEFAULT
    w16(base, s + 6,  1);          // st_shndx: non-zero (not SHN_UNDEF)
    w64(base, s + 8,  value);      // st_value: offset from load base
    w64(base, s + 16, size);       // st_size
}

unsafe fn write_symtab(base: *mut u8) {
    // Symbol 0: null (required by ELF spec) — already zeroed.
    // Symbols 1-7: our vDSO functions.
    //   strtab offsets: 1, 22, 42, 56, 70, 84, 103
    write_sym(base, 1,   1, OFF_CLOCK_GETTIME as u64, 84);
    write_sym(base, 2,  22, OFF_GETTIMEOFDAY as u64,   8);
    write_sym(base, 3,  42, OFF_GETPID as u64,         7);
    write_sym(base, 4,  56, OFF_GETTID as u64,        19);
    write_sym(base, 5,  70, OFF_GETUID as u64,         7);
    write_sym(base, 6,  84, OFF_GETPRIORITY as u64,   28);
    write_sym(base, 7, 103, OFF_UNAME as u64,         17);
}

// ── String Table ────────────────────────────────────────────────────
// Offsets: 0:\0  1:__vdso_clock_gettime\0  22:__vdso_gettimeofday\0
//          42:__vdso_getpid\0  56:__vdso_gettid\0  70:__vdso_getuid\0
//          84:__vdso_getpriority\0  103:__vdso_uname\0
// Total: 116 bytes.

const STRTAB: &[u8] = b"\0__vdso_clock_gettime\0__vdso_gettimeofday\0__vdso_getpid\0__vdso_gettid\0__vdso_getuid\0__vdso_getpriority\0__vdso_uname\0";

unsafe fn write_strtab(base: *mut u8) {
    wbytes(base, OFF_STRTAB, STRTAB);
}

// ── SYSV Hash Table ────────────────────────────────────────────────
//
// nbucket=1, nchain=8.  All symbols hash to bucket 0.
// musl iterates bucket → chain until chain[i] == 0.
//
// Layout: [1, 8, 1, 0, 2, 3, 4, 5, 6, 7, 0]
//          nb nc b0 c0 c1 c2 c3 c4 c5 c6 c7

unsafe fn write_hash(base: *mut u8) {
    let h = OFF_HASH;
    w32(base, h,       1);   // nbucket
    w32(base, h +  4,  8);   // nchain (number of symbols)
    w32(base, h +  8,  1);   // bucket[0] → symbol index 1
    w32(base, h + 12,  0);   // chain[0] → end (null symbol)
    w32(base, h + 16,  2);   // chain[1] → next: symbol 2
    w32(base, h + 20,  3);   // chain[2] → next: symbol 3
    w32(base, h + 24,  4);   // chain[3] → next: symbol 4
    w32(base, h + 28,  5);   // chain[4] → next: symbol 5
    w32(base, h + 32,  6);   // chain[5] → next: symbol 6
    w32(base, h + 36,  7);   // chain[6] → next: symbol 7
    w32(base, h + 40,  0);   // chain[7] → end
}

// ── Code ────────────────────────────────────────────────────────────
//
// All functions use RIP-relative addressing to read data at 0xE00+.
// Displacements are computed as: target_addr - (instruction_end_addr).

// __vdso_clock_gettime at 0x300 (84 bytes)
// Handles CLOCK_REALTIME (0) and CLOCK_MONOTONIC (1) via rdtsc.
// CLOCK_REALTIME adds wall_epoch_ns (RTC boot epoch) to monotonic nanoseconds.
// Falls back to syscall for all other clock IDs.
// RIP-relative targets:
//   sub rax,[rip+disp] ends at 0x319 → disp = 0xE00-0x319 = 0xAE7 (tsc_origin)
//   mov rcx,[rip+disp] ends at 0x320 → disp = 0xE08-0x320 = 0xAE8 (ns_mult)
//   add rax,[rip+disp] ends at 0x333 → disp = 0xE10-0x333 = 0xADD (wall_epoch_ns)
#[rustfmt::skip]
const CODE_CLOCK_GETTIME: [u8; 84] = [
    0x83, 0xff, 0x01,                          // cmp edi, 1           (CLOCK_MONOTONIC?)
    0x74, 0x04,                                // je .monotonic
    0x85, 0xff,                                // test edi, edi        (CLOCK_REALTIME?)
    0x75, 0x43,                                // jnz .fallback
    // .monotonic:
    0x0f, 0x31,                                 // rdtsc
    0x48, 0xc1, 0xe2, 0x20,                     // shl rdx, 32
    0x48, 0x09, 0xd0,                           // or rax, rdx
    0x48, 0x2b, 0x05, 0xe7, 0x0a, 0x00, 0x00,  // sub rax, [rip+0xae7] → tsc_origin
    0x48, 0x8b, 0x0d, 0xe8, 0x0a, 0x00, 0x00,  // mov rcx, [rip+0xae8] → ns_mult
    0x48, 0xf7, 0xe1,                           // mul rcx
    0x48, 0x0f, 0xac, 0xd0, 0x20,               // shrd rax, rdx, 32
    // RAX = monotonic nanoseconds since boot.
    0x85, 0xff,                                 // test edi, edi  (was REALTIME?)
    0x75, 0x07,                                 // jnz .store     (skip epoch add for MONOTONIC)
    0x48, 0x03, 0x05, 0xdd, 0x0a, 0x00, 0x00,  // add rax, [rip+0xadd] → wall_epoch_ns
    // .store:
    0x48, 0xb9, 0x00, 0xca, 0x9a, 0x3b, 0x00, 0x00, 0x00, 0x00, // mov rcx, 1000000000
    0x31, 0xd2,                                 // xor edx, edx
    0x48, 0xf7, 0xf1,                           // div rcx
    0x48, 0x89, 0x06,                           // mov [rsi], rax       (tv_sec)
    0x48, 0x89, 0x56, 0x08,                     // mov [rsi+8], rdx     (tv_nsec)
    0x31, 0xc0,                                 // xor eax, eax
    0xc3,                                       // ret
    // .fallback:
    0xb8, 0xe4, 0x00, 0x00, 0x00,               // mov eax, 228 (SYS_clock_gettime)
    0x0f, 0x05,                                 // syscall
    0xc3,                                       // ret
];

// __vdso_gettimeofday at 0x354 (8 bytes)
// Syscall fallback — gettimeofday is not performance-critical.
// musl's gettimeofday() internally calls clock_gettime(REALTIME) via vDSO
// when available, so the fast path is covered by __vdso_clock_gettime above.
#[rustfmt::skip]
const CODE_GETTIMEOFDAY: [u8; 8] = [
    0xb8, 0x60, 0x00, 0x00, 0x00,               // mov eax, 96 (SYS_gettimeofday)
    0x0f, 0x05,                                 // syscall
    0xc3,                                       // ret
];

// __vdso_getpid at 0x35C (7 bytes)
//   mov eax,[rip+disp] ends at 0x362 → disp = 0xE18-0x362 = 0xAB6
#[rustfmt::skip]
const CODE_GETPID: [u8; 7] = [
    0x8b, 0x05, 0xb6, 0x0a, 0x00, 0x00,        // mov eax, [rip+0xab6] → pid
    0xc3,                                       // ret
];

// __vdso_gettid at 0x363 (19 bytes)
// If tid==0, fall back to syscall (multi-threaded process).
//   mov eax,[rip+disp] ends at 0x369 → disp = 0xE1C-0x369 = 0xAB3
#[rustfmt::skip]
const CODE_GETTID: [u8; 19] = [
    0x8b, 0x05, 0xb3, 0x0a, 0x00, 0x00,        // mov eax, [rip+0xab3] → tid
    0x85, 0xc0,                                 // test eax, eax
    0x74, 0x01,                                 // jz .syscall
    0xc3,                                       // ret
    0xb8, 0xba, 0x00, 0x00, 0x00,               // .syscall: mov eax, 186
    0x0f, 0x05,                                 // syscall
    0xc3,                                       // ret
];

// __vdso_getuid at 0x376 (7 bytes)
//   mov eax,[rip+disp] ends at 0x37C → disp = 0xE20-0x37C = 0xAA4
#[rustfmt::skip]
const CODE_GETUID: [u8; 7] = [
    0x8b, 0x05, 0xa4, 0x0a, 0x00, 0x00,        // mov eax, [rip+0xaa4] → uid
    0xc3,                                       // ret
];

// __vdso_getpriority at 0x37D (28 bytes)
// Fast path: which==PRIO_PROCESS(0) && who==0 → return 20 - nice.
// Otherwise fall back to syscall.
//   sub eax,[rip+disp] ends at 0x390 → disp = 0xE24-0x390 = 0xA94
#[rustfmt::skip]
const CODE_GETPRIORITY: [u8; 28] = [
    0x85, 0xff,                                 // test edi, edi
    0x75, 0x10,                                 // jnz .syscall (rel8=0x10)
    0x85, 0xf6,                                 // test esi, esi
    0x75, 0x0c,                                 // jnz .syscall (rel8=0x0c)
    0xb8, 0x14, 0x00, 0x00, 0x00,               // mov eax, 20
    0x2b, 0x05, 0x94, 0x0a, 0x00, 0x00,         // sub eax, [rip+0xa94] → nice
    0xc3,                                       // ret
    0xb8, 0x8c, 0x00, 0x00, 0x00,               // .syscall: mov eax, 140
    0x0f, 0x05,                                 // syscall
    0xc3,                                       // ret
];

// __vdso_uname at 0x399 (17 bytes)
// Copy 390 bytes from embedded utsname data to [rdi].
//   lea rsi,[rip+disp] ends at 0x3A0 → disp = 0xE28-0x3A0 = 0xA88
#[rustfmt::skip]
const CODE_UNAME: [u8; 17] = [
    0x48, 0x8d, 0x35, 0x88, 0x0a, 0x00, 0x00,  // lea rsi, [rip+0xa88] → utsname
    0xb9, 0x86, 0x01, 0x00, 0x00,               // mov ecx, 390
    0xf3, 0xa4,                                 // rep movsb
    0x31, 0xc0,                                 // xor eax, eax
    0xc3,                                       // ret
];

unsafe fn write_code(base: *mut u8) {
    wbytes(base, OFF_CLOCK_GETTIME, &CODE_CLOCK_GETTIME);
    wbytes(base, OFF_GETTIMEOFDAY,  &CODE_GETTIMEOFDAY);
    wbytes(base, OFF_GETPID,        &CODE_GETPID);
    wbytes(base, OFF_GETTID,        &CODE_GETTID);
    wbytes(base, OFF_GETUID,        &CODE_GETUID);
    wbytes(base, OFF_GETPRIORITY,   &CODE_GETPRIORITY);
    wbytes(base, OFF_UNAME,         &CODE_UNAME);
}

// ── Data area (TSC parameters from boot, per-process fields zeroed) ──

unsafe fn write_data(base: *mut u8) {
    let origin = super::tsc::tsc_origin();
    let mult = super::tsc::ns_mult();
    w64(base, OFF_TSC_ORIGIN, origin);
    w64(base, OFF_NS_MULT, mult);
    // Wall-clock epoch: RTC seconds at boot → nanoseconds.
    let epoch_secs = super::read_rtc_epoch_secs();
    w64(base, OFF_WALL_EPOCH_NS, epoch_secs * 1_000_000_000);
    // Per-process fields (pid, tid, uid, nice, utsname) left zeroed in template.
}
