// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! ISA debugcon driver for QEMU high-bandwidth binary output.
//!
//! QEMU's ISA debugcon device maps an I/O port (default 0xe9) to a host-side
//! chardev (file, pipe, socket). Each `outb` writes one byte at ~200ns on KVM,
//! giving ~5 MB/s throughput — 350x faster than serial at 115200 baud.
//!
//! # QEMU flags
//! ```text
//! -chardev file,id=ktrace,path=ktrace.bin -device isa-debugcon,chardev=ktrace,ioport=0xe9
//! ```

/// The I/O port used by ISA debugcon.
const DEBUGCON_PORT: u16 = 0xe9;

/// Write a single byte to the debugcon port.
#[inline(always)]
#[allow(unsafe_code)]
pub fn write_byte(byte: u8) {
    unsafe {
        x86::io::outb(DEBUGCON_PORT, byte);
    }
}

/// Write a slice of bytes to the debugcon port.
///
/// Each byte is a separate `outb` — no buffering needed since QEMU's chardev
/// handles it. On KVM this runs at ~200ns/byte = ~5 MB/s.
#[inline(never)]
pub fn write_bytes(data: &[u8]) {
    for &b in data {
        write_byte(b);
    }
}

/// Write a u32 in little-endian to the debugcon port.
#[inline(always)]
pub fn write_u32(val: u32) {
    write_bytes(&val.to_le_bytes());
}

/// Write a u64 in little-endian to the debugcon port.
#[inline(always)]
pub fn write_u64(val: u64) {
    write_bytes(&val.to_le_bytes());
}
