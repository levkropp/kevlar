// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ARM64 ktrace transport via ARM semihosting.
//!
//! The ARM semihosting protocol lets a guest write to the host through a
//! single trap instruction (`HLT #0xF000`), giving QEMU a chance to redirect
//! output to a chardev file — the exact same role that ISA debugcon's `outb`
//! fills on x86_64.  QEMU enables this with:
//!
//! ```text
//! -semihosting-config enable=on,target=native,chardev=ktrace
//! -chardev file,id=ktrace,path=ktrace.bin
//! ```
//!
//! # Operation mapping
//!
//! | x86_64               | ARM64                          |
//! |----------------------|-------------------------------|
//! | `outb(0xe9, byte)`   | `SYS_WRITEC` via HLT #0xF000  |
//! | `outb` ×N            | `SYS_WRITE` (one trap, N bytes)|
//!
//! `write_byte` uses **SYS_WRITEC** (op 0x03): one trap per byte, lowest
//! latency for single-byte calls.  `write_bytes` uses **SYS_WRITE** (op
//! 0x05): one trap per slice, making the ktrace ring-buffer dump as fast as
//! a single QEMU semihosting call regardless of buffer size.
//!
//! # Performance
//!
//! On TCG (no KVM) a semihosting trap is ~500 ns.  For typical ktrace dumps
//! (256 KB, one `write_bytes` call) that is a single trap, comparable to the
//! single DMA transfer that a real hardware trace port would perform.
//!
//! # ARM semihosting protocol (AArch64)
//!
//! Caller sets `x0 = op`, `x1 = param_block_ptr`, then executes `HLT #0xF000`.
//! QEMU intercepts the debug exception and performs the operation.
//! On return, `x0` contains the result (0 = success for SYS_WRITE).
//!
//! SYS_WRITEC param: `x1` = pointer to byte
//! SYS_WRITE param:  `x1` = pointer to `[handle: u64, data: *const u8, len: u64]`

/// Semihosting operation: write single character to the debug channel.
const SYS_WRITEC: usize = 0x03;
/// Semihosting operation: write buffer to an open file handle.
const SYS_WRITE: usize = 0x05;
/// Semihosting file-handle for stderr — routed to the ktrace chardev.
const STDERR_HANDLE: usize = 2;

/// Write a single byte to the ktrace semihosting channel.
///
/// Uses `SYS_WRITEC` (one trap per byte).  Prefer [`write_bytes`] for bulk
/// data — it completes a whole slice with a single trap.
#[inline(always)]
#[allow(unsafe_code)]
pub fn write_byte(byte: u8) {
    // SYS_WRITEC: x0=3, x1=&byte, HLT #0xF000.
    // The byte is on the stack; &byte is valid for the duration of the trap.
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            in("x0") SYS_WRITEC,
            in("x1") &byte as *const u8,
            lateout("x0") _,    // semihosting may clobber x0 on return
            options(nostack),
        );
    }
}

/// Write a byte slice to the ktrace semihosting channel in a single trap.
///
/// Uses `SYS_WRITE` with a stack-allocated parameter block.  The entire slice
/// is consumed by one `HLT #0xF000` trap regardless of length — much more
/// efficient than `write_byte` in a loop when dumping ring buffers.
#[inline(never)]
#[allow(unsafe_code)]
pub fn write_bytes(data: &[u8]) {
    if data.is_empty() {
        return;
    }
    // SYS_WRITE parameter block (AArch64 = 64-bit words):
    //   word 0: file handle (2 = stderr → ktrace chardev)
    //   word 1: pointer to data buffer
    //   word 2: byte count
    let params: [usize; 3] = [STDERR_HANDLE, data.as_ptr() as usize, data.len()];
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            in("x0") SYS_WRITE,
            in("x1") params.as_ptr(),
            lateout("x0") _,
            options(nostack, readonly),
        );
    }
}

/// Write a `u32` in little-endian byte order.
#[inline(always)]
pub fn write_u32(val: u32) {
    write_bytes(&val.to_le_bytes());
}

/// Write a `u64` in little-endian byte order.
#[inline(always)]
pub fn write_u64(val: u64) {
    write_bytes(&val.to_le_bytes());
}
