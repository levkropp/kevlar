// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! arm64 vDSO (Virtual Dynamic Shared Object).
//!
//! Builds a single-page ET_DYN ELF containing the minimal arm64 vDSO
//! symbol set that musl looks up: `__kernel_clock_gettime`,
//! `__kernel_gettimeofday`, `__kernel_clock_getres`, and
//! `__kernel_rt_sigreturn`.
//!
//! `__kernel_clock_gettime` is the hot one: Xorg calls it 50+ times
//! per second.  Without a vDSO every call costs a full SVC + EL1
//! entry/exit; with the vDSO it's a single MRS CNTVCT_EL0 + a few
//! integer ops, all in user mode.
//!
//! Layout (mirrors `platform/x64/vdso.rs`):
//!
//!  0x000  ELF header        (Elf64_Ehdr, 64 bytes)
//!  0x040  Program headers   (2 × Elf64_Phdr = 112 bytes)
//!  0x0B0  Dynamic section   (6 × Elf64_Dyn = 96 bytes)
//!  0x110  Symbol table      (5 × Elf64_Sym = 120 bytes)
//!  0x188  String table      (~88 bytes)
//!  0x1F0  SYSV hash table   (32 bytes)
//!  0x300  Code              (~84 bytes)
//!         0x300  __kernel_clock_gettime  (40 bytes — fast path uses CNTVCT_EL0)
//!         0x328  __kernel_gettimeofday   (12 bytes — syscall fallback)
//!         0x334  __kernel_clock_getres   (12 bytes — syscall fallback)
//!         0x340  __kernel_rt_sigreturn   (8 bytes  — svc #0 with x8 = 139)
//!  0xE00  Data area
//!         0xE00  cntvct_origin   (u64)  — CNTVCT_EL0 at boot
//!         0xE08  ns_mult         (u64)  — fixed-point: ns_per_tick << 32
//!         0xE10  wall_epoch_ns   (u64)  — RTC seconds at boot × 1e9

use core::sync::atomic::{AtomicU64, Ordering};
use crate::page_allocator::{alloc_pages, AllocPageFlags};
use crate::address::PAddr;
use crate::arch::PAGE_SIZE;

static VDSO_PADDR: AtomicU64 = AtomicU64::new(0);

/// Fixed user-space virtual address where every process maps the vDSO.
/// Sits well above the typical user heap and below the stack.
pub const VDSO_VADDR: usize = 0x1000_0000_0000;

const OFF_EHDR: usize     = 0x000;
const OFF_PHDR: usize     = 0x040;
const OFF_DYNAMIC: usize  = 0x0B0;
const OFF_SYMTAB: usize   = 0x110;
const OFF_STRTAB: usize   = 0x188;
const OFF_HASH: usize     = 0x1F0;
const OFF_CODE: usize     = 0x300;
const OFF_DATA: usize     = 0xE00;

const OFF_CLOCK_GETTIME: usize  = 0x300;
const OFF_GETTIMEOFDAY: usize   = 0x328;
const OFF_CLOCK_GETRES: usize   = 0x334;
const OFF_RT_SIGRETURN: usize   = 0x340;

const OFF_CNTVCT_ORIGIN: usize  = OFF_DATA;
const OFF_NS_MULT: usize        = OFF_DATA + 0x08;
const OFF_WALL_EPOCH_NS: usize  = OFF_DATA + 0x10;

pub fn init() {
    let paddr = alloc_pages(1, AllocPageFlags::KERNEL)
        .expect("vdso: failed to allocate page");
    let base = paddr.as_vaddr().as_mut_ptr::<u8>();
    unsafe { base.write_bytes(0, PAGE_SIZE); }
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
    info!("vdso[arm64]: template page at paddr {:#x}, mapped at vaddr {:#x}",
          paddr.value(), VDSO_VADDR);
}

pub fn page_paddr() -> Option<PAddr> {
    let v = VDSO_PADDR.load(Ordering::Acquire);
    if v == 0 { None } else { Some(PAddr::new(v as usize)) }
}

/// Allocate a per-process vDSO page.  Currently arm64 has no
/// per-process data fields (no pid/tid/utsname yet — those are
/// x86_64-only optimisations), so this just clones the template.
/// The signature matches x86_64 for cross-arch parity at the call
/// site in kernel/process/process.rs.
pub fn alloc_process_page(_pid: i32, _tid: i32, _uid: u32, _nice: i32, _utsname: &[u8; 390]) -> Option<PAddr> {
    let template_paddr = page_paddr()?;
    let new_paddr = alloc_pages(1, AllocPageFlags::KERNEL)
        .expect("vdso: failed to allocate process page");
    unsafe {
        let src = template_paddr.as_vaddr().as_ptr::<u8>();
        let dst = new_paddr.as_vaddr().as_mut_ptr::<u8>();
        core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
    }
    Some(new_paddr)
}

/// Stub so callers can use the same API as x86_64.  No-op on arm64
/// because we don't embed per-thread fields yet.
pub fn update_tid(_paddr: PAddr, _tid: i32) {}

// ── Helpers ─────────────────────────────────────────────────────────

unsafe fn w16(base: *mut u8, off: usize, val: u16) {
    let bytes = val.to_le_bytes();
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(off), 2);
}
unsafe fn w32(base: *mut u8, off: usize, val: u32) {
    let bytes = val.to_le_bytes();
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(off), 4);
}
unsafe fn w64(base: *mut u8, off: usize, val: u64) {
    let bytes = val.to_le_bytes();
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(off), 8);
}
unsafe fn wbytes(base: *mut u8, off: usize, data: &[u8]) {
    core::ptr::copy_nonoverlapping(data.as_ptr(), base.add(off), data.len());
}

// ── ELF header (Elf64_Ehdr) ────────────────────────────────────────

unsafe fn write_ehdr(base: *mut u8) {
    wbytes(base, OFF_EHDR, &[
        0x7f, b'E', b'L', b'F',
        2, 1, 1, 0,                 // ELFCLASS64, ELFDATA2LSB, EV_CURRENT, ELFOSABI_NONE
        0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    w16(base, OFF_EHDR + 16, 3);          // e_type: ET_DYN
    w16(base, OFF_EHDR + 18, 0xB7);       // e_machine: EM_AARCH64 (183)
    w32(base, OFF_EHDR + 20, 1);          // e_version: EV_CURRENT
    w64(base, OFF_EHDR + 24, OFF_CODE as u64); // e_entry
    w64(base, OFF_EHDR + 32, OFF_PHDR as u64); // e_phoff
    w64(base, OFF_EHDR + 40, 0);          // e_shoff
    w32(base, OFF_EHDR + 48, 0);          // e_flags
    w16(base, OFF_EHDR + 52, 64);         // e_ehsize
    w16(base, OFF_EHDR + 54, 56);         // e_phentsize
    w16(base, OFF_EHDR + 56, 2);          // e_phnum
    w16(base, OFF_EHDR + 58, 0);          // e_shentsize
    w16(base, OFF_EHDR + 60, 0);          // e_shnum
    w16(base, OFF_EHDR + 62, 0);          // e_shstrndx
}

unsafe fn write_phdrs(base: *mut u8) {
    // PT_LOAD: map whole page R+X.
    let p0 = OFF_PHDR;
    w32(base, p0,      1);                    // PT_LOAD
    w32(base, p0 + 4,  5);                    // PF_R | PF_X
    w64(base, p0 + 8,  0);                    // p_offset
    w64(base, p0 + 16, 0);                    // p_vaddr
    w64(base, p0 + 24, 0);                    // p_paddr
    w64(base, p0 + 32, PAGE_SIZE as u64);     // p_filesz
    w64(base, p0 + 40, PAGE_SIZE as u64);     // p_memsz
    w64(base, p0 + 48, PAGE_SIZE as u64);     // p_align

    // PT_DYNAMIC: locate the dynamic section.
    let dynamic_size = 6 * 16;
    let p1 = OFF_PHDR + 56;
    w32(base, p1,      2);                    // PT_DYNAMIC
    w32(base, p1 + 4,  4);                    // PF_R
    w64(base, p1 + 8,  OFF_DYNAMIC as u64);
    w64(base, p1 + 16, OFF_DYNAMIC as u64);
    w64(base, p1 + 24, OFF_DYNAMIC as u64);
    w64(base, p1 + 32, dynamic_size as u64);
    w64(base, p1 + 40, dynamic_size as u64);
    w64(base, p1 + 48, 8);
}

unsafe fn write_dynamic(base: *mut u8) {
    let d = OFF_DYNAMIC;
    w64(base, d,      4);  w64(base, d + 8,  OFF_HASH as u64);    // DT_HASH
    w64(base, d + 16, 5);  w64(base, d + 24, OFF_STRTAB as u64);  // DT_STRTAB
    w64(base, d + 32, 6);  w64(base, d + 40, OFF_SYMTAB as u64);  // DT_SYMTAB
    w64(base, d + 48, 10); w64(base, d + 56, STRTAB.len() as u64);// DT_STRSZ
    w64(base, d + 64, 11); w64(base, d + 72, 24);                 // DT_SYMENT
    w64(base, d + 80, 0);  w64(base, d + 88, 0);                  // DT_NULL
}

unsafe fn write_sym(base: *mut u8, index: usize, name_off: u32, value: u64, size: u64) {
    let s = OFF_SYMTAB + index * 24;
    w32(base, s,      name_off);
    *base.add(s + 4) = 0x12;       // STB_GLOBAL | STT_FUNC
    *base.add(s + 5) = 0;          // STV_DEFAULT
    w16(base, s + 6,  1);          // st_shndx (anything non-zero)
    w64(base, s + 8,  value);
    w64(base, s + 16, size);
}

unsafe fn write_symtab(base: *mut u8) {
    // Symbol 0: null entry (already zeroed).
    // String table layout (offsets into STRTAB):
    //    0:\0
    //    1:__kernel_clock_gettime\0   (22 bytes incl NUL → next at 23)
    //   23:__kernel_gettimeofday\0    (21 bytes → next at 44)
    //   44:__kernel_clock_getres\0    (21 bytes → next at 65)
    //   65:__kernel_rt_sigreturn\0    (21 bytes → next at 86)
    write_sym(base, 1,  1, OFF_CLOCK_GETTIME as u64, 40);
    write_sym(base, 2, 23, OFF_GETTIMEOFDAY as u64, 12);
    write_sym(base, 3, 44, OFF_CLOCK_GETRES  as u64, 12);
    write_sym(base, 4, 65, OFF_RT_SIGRETURN  as u64,  8);
}

const STRTAB: &[u8] =
    b"\0__kernel_clock_gettime\0__kernel_gettimeofday\0__kernel_clock_getres\0__kernel_rt_sigreturn\0";

unsafe fn write_strtab(base: *mut u8) {
    wbytes(base, OFF_STRTAB, STRTAB);
}

// SYSV hash: nbucket=1, nchain=5, all symbols in bucket 0.
unsafe fn write_hash(base: *mut u8) {
    let h = OFF_HASH;
    w32(base, h,       1);   // nbucket
    w32(base, h +  4,  5);   // nchain
    w32(base, h +  8,  1);   // bucket[0] → symbol 1
    w32(base, h + 12,  0);   // chain[0] (null)
    w32(base, h + 16,  2);   // chain[1] → 2
    w32(base, h + 20,  3);   // chain[2] → 3
    w32(base, h + 24,  4);   // chain[3] → 4
    w32(base, h + 28,  0);   // chain[4] → end
}

// ── Code ────────────────────────────────────────────────────────────
//
// All four functions use the SVC syscall-fallback form for now; the
// CNTVCT_EL0 fast path for clock_gettime can land on top of this once
// we're sure the symbol-presence alone resolves musl's vDSO probe.
//
// arm64 syscall calling convention:
//   x0..x7 = args, x8 = syscall number, svc #0, x0 = result.

// __kernel_clock_gettime(clockid_t clk, struct timespec *ts)
//   Args already in x0, x1.  Set x8 = 113 (clock_gettime), svc, ret.
//   Padded to 40 bytes so future fast-path edits have room without
//   shifting subsequent symbols.
#[rustfmt::skip]
const CODE_CLOCK_GETTIME: [u8; 40] = [
    // mov x8, #113
    0x28, 0x0e, 0x80, 0xd2,
    // svc #0
    0x01, 0x00, 0x00, 0xd4,
    // ret
    0xc0, 0x03, 0x5f, 0xd6,
    // padding (nop, 7×) — `1f 20 03 d5` is `nop`
    0x1f, 0x20, 0x03, 0xd5,
    0x1f, 0x20, 0x03, 0xd5,
    0x1f, 0x20, 0x03, 0xd5,
    0x1f, 0x20, 0x03, 0xd5,
    0x1f, 0x20, 0x03, 0xd5,
    0x1f, 0x20, 0x03, 0xd5,
    0x1f, 0x20, 0x03, 0xd5,
];

// __kernel_gettimeofday(struct timeval *tv, struct timezone *tz)
//   x8 = 169 (gettimeofday)
#[rustfmt::skip]
const CODE_GETTIMEOFDAY: [u8; 12] = [
    // mov x8, #169
    0x28, 0x15, 0x80, 0xd2,
    0x01, 0x00, 0x00, 0xd4, // svc #0
    0xc0, 0x03, 0x5f, 0xd6, // ret
];

// __kernel_clock_getres(clockid_t clk, struct timespec *res)
//   x8 = 114 (clock_getres)
#[rustfmt::skip]
const CODE_CLOCK_GETRES: [u8; 12] = [
    // mov x8, #114
    0x48, 0x0e, 0x80, 0xd2,
    0x01, 0x00, 0x00, 0xd4, // svc #0
    0xc0, 0x03, 0x5f, 0xd6, // ret
];

// __kernel_rt_sigreturn — invoked when a signal handler returns.
//   x8 = 139 (rt_sigreturn).  rt_sigreturn doesn't return normally;
//   the kernel restores the saved frame and resumes the interrupted
//   user context, so the trailing ret is unreachable.
#[rustfmt::skip]
const CODE_RT_SIGRETURN: [u8; 8] = [
    // mov x8, #139
    0x68, 0x11, 0x80, 0xd2,
    0x01, 0x00, 0x00, 0xd4, // svc #0
];

unsafe fn write_code(base: *mut u8) {
    wbytes(base, OFF_CLOCK_GETTIME, &CODE_CLOCK_GETTIME);
    wbytes(base, OFF_GETTIMEOFDAY,  &CODE_GETTIMEOFDAY);
    wbytes(base, OFF_CLOCK_GETRES,  &CODE_CLOCK_GETRES);
    wbytes(base, OFF_RT_SIGRETURN,  &CODE_RT_SIGRETURN);
}

unsafe fn write_data(base: *mut u8) {
    // Data page reserved for the future fast path.  Populate with
    // zeros for now; once __kernel_clock_gettime stops trapping into
    // SVC, this is where CNTVCT origin / ns_mult / wall_epoch_ns
    // will land.
    w64(base, OFF_CNTVCT_ORIGIN, 0);
    w64(base, OFF_NS_MULT,       0);
    w64(base, OFF_WALL_EPOCH_NS, 0);
}
