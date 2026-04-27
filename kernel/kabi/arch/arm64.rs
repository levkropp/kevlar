// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! aarch64 ELF relocation handlers for kABI module loading.
//!
//! Reference: ARM IHI 0056 — ELF for the Arm 64-bit Architecture
//! (AArch64), §4.6.4 Relocation operations.
//!
//! Handles the minimum set a `printk`-calling
//! `-mcmodel=tiny -fno-pic` ET_REL emits.  Fail-loud panic on any
//! reloc kind we don't know about — K2+ widens this match arm-by-arm
//! as new modules surface new types.

use crate::result::Result;

// Subset of R_AARCH64_* constants (mirrors goblin's
// constants_relocation; redeclared here to keep the dispatcher
// self-contained).
const R_AARCH64_NONE: u32 = 0;
const R_AARCH64_ABS64: u32 = 257;
const R_AARCH64_ABS32: u32 = 258;
const R_AARCH64_PREL64: u32 = 260;
const R_AARCH64_PREL32: u32 = 261;
const R_AARCH64_ADR_PREL_LO21: u32 = 274;
const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;
const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;
const R_AARCH64_LDST8_ABS_LO12_NC: u32 = 278;
const R_AARCH64_LDST16_ABS_LO12_NC: u32 = 284;
const R_AARCH64_LDST32_ABS_LO12_NC: u32 = 285;
const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;
const R_AARCH64_LDST128_ABS_LO12_NC: u32 = 299;
const R_AARCH64_CALL26: u32 = 283;
const R_AARCH64_JUMP26: u32 = 282;

/// Apply a single relocation.
#[allow(unsafe_code)]
pub fn apply(r_type: u32, target: usize, sym_va: usize, addend: i64) -> Result<()> {
    let s = sym_va as i64;
    let a = addend;
    let p = target as i64;

    match r_type {
        R_AARCH64_NONE => {}

        R_AARCH64_ABS64 => {
            let val = (s + a) as u64;
            unsafe { core::ptr::write_unaligned(target as *mut u64, val) };
        }
        R_AARCH64_ABS32 => {
            let val = (s + a) as u32;
            unsafe { core::ptr::write_unaligned(target as *mut u32, val) };
        }
        R_AARCH64_PREL64 => {
            let val = (s + a - p) as u64;
            unsafe { core::ptr::write_unaligned(target as *mut u64, val) };
        }
        R_AARCH64_PREL32 => {
            let val = (s + a - p) as u32;
            unsafe { core::ptr::write_unaligned(target as *mut u32, val) };
        }

        // bl / b imm26 — bits [25:0] hold (S+A−P)>>2, range ±128MB.
        R_AARCH64_CALL26 | R_AARCH64_JUMP26 => {
            let off = s + a - p;
            // Range check: ±128MB.
            if off < -(128 << 20) || off >= (128 << 20) {
                panic!(
                    "kabi: R_AARCH64_CALL26/JUMP26 out of ±128MB range \
                     (sym={:#x} target={:#x} off={})",
                    sym_va, target, off
                );
            }
            if off & 3 != 0 {
                panic!("kabi: R_AARCH64_CALL26 unaligned offset {}", off);
            }
            let imm26 = ((off >> 2) as u32) & 0x03ff_ffff;
            let insn = unsafe { core::ptr::read_unaligned(target as *const u32) };
            let new_insn = (insn & !0x03ff_ffff) | imm26;
            unsafe { core::ptr::write_unaligned(target as *mut u32, new_insn) };
        }

        // adr imm21 — bits [30:29] (immlo) and [23:5] (immhi) hold
        // the byte offset (S+A−P), range ±1MB.  Emitted by
        // `-mcmodel=tiny` for local references (e.g. .rodata strings
        // co-located with .text in the same module image).
        R_AARCH64_ADR_PREL_LO21 => {
            let off = s + a - p;
            if off < -(1 << 20) || off >= (1 << 20) {
                panic!(
                    "kabi: R_AARCH64_ADR_PREL_LO21 out of ±1MB range \
                     (sym={:#x} target={:#x} off={})",
                    sym_va, target, off
                );
            }
            let imm21 = (off as u32) & 0x1f_ffff;
            let immlo = (imm21 & 0x3) << 29;
            let immhi = ((imm21 >> 2) & 0x7_ffff) << 5;
            let insn = unsafe { core::ptr::read_unaligned(target as *const u32) };
            let mask = !((0x3 << 29) | (0x7_ffff << 5));
            let new_insn = (insn & mask) | immlo | immhi;
            unsafe { core::ptr::write_unaligned(target as *mut u32, new_insn) };
        }

        // adrp imm21 — bits [30:29] (immlo) and [23:5] (immhi)
        // hold pageof(S+A) − pageof(P).
        R_AARCH64_ADR_PREL_PG_HI21 => {
            let target_page = ((s + a) as u64) & !0xfffu64;
            let pc_page = (p as u64) & !0xfffu64;
            let off = (target_page as i64) - (pc_page as i64);
            // 21-bit signed page offset → ±4GB after <<12.
            let pages = off >> 12;
            if pages < -(1 << 20) || pages >= (1 << 20) {
                panic!(
                    "kabi: R_AARCH64_ADR_PREL_PG_HI21 out of ±4GB range \
                     (sym={:#x} target={:#x} pages={})",
                    sym_va, target, pages
                );
            }
            let imm21 = (pages as u32) & 0x1f_ffff;
            let immlo = (imm21 & 0x3) << 29;
            let immhi = ((imm21 >> 2) & 0x7_ffff) << 5;
            let insn = unsafe { core::ptr::read_unaligned(target as *const u32) };
            let mask = !((0x3 << 29) | (0x7_ffff << 5));
            let new_insn = (insn & mask) | immlo | immhi;
            unsafe { core::ptr::write_unaligned(target as *mut u32, new_insn) };
        }

        // add x?, x?, #imm12 — bits [21:10] hold (S+A) & 0xfff.
        // Same encoding for ldst lo12_nc forms.
        R_AARCH64_ADD_ABS_LO12_NC
        | R_AARCH64_LDST8_ABS_LO12_NC
        | R_AARCH64_LDST16_ABS_LO12_NC
        | R_AARCH64_LDST32_ABS_LO12_NC
        | R_AARCH64_LDST64_ABS_LO12_NC
        | R_AARCH64_LDST128_ABS_LO12_NC => {
            let val = ((s + a) as u64) & 0xfff;
            // For LDST instructions, the imm12 is naturally aligned
            // (LDR x = >>3, LDR w = >>2, LDRH = >>1, LDRB = >>0).  The
            // _NC ("no check") variants intentionally don't enforce
            // alignment of the symbol, but the encoding still expects
            // the unscaled 12-bit value to be shifted appropriately.
            // For our printk-calling demo we only hit ADD_ABS_LO12_NC
            // (string-literal address fixup); the others are here so
            // K2 modules don't immediately trip the panic.
            let shift = match r_type {
                R_AARCH64_ADD_ABS_LO12_NC => 0,
                R_AARCH64_LDST8_ABS_LO12_NC => 0,
                R_AARCH64_LDST16_ABS_LO12_NC => 1,
                R_AARCH64_LDST32_ABS_LO12_NC => 2,
                R_AARCH64_LDST64_ABS_LO12_NC => 3,
                R_AARCH64_LDST128_ABS_LO12_NC => 4,
                _ => 0,
            };
            let imm12 = ((val >> shift) as u32) & 0xfff;
            let insn = unsafe { core::ptr::read_unaligned(target as *const u32) };
            let new_insn = (insn & !(0xfff << 10)) | (imm12 << 10);
            unsafe { core::ptr::write_unaligned(target as *mut u32, new_insn) };
        }

        _ => panic!(
            "kabi: unhandled aarch64 reloc R_AARCH64_{} at module+{:#x} \
             (sym={:#x}, addend={})",
            r_type, target, sym_va, addend
        ),
    }
    Ok(())
}
