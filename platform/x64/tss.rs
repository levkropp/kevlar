// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use super::gdt::TSS_SEG;
use x86::{segmentation::SegmentSelector, task::load_tr};

pub const IST_RSP0: u8 = 0;

#[derive(Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_offset: u16,
    iomap: [u8; 8191],
    iomap_last_byte: u8,
}

impl Tss {
    pub fn set_rsp0(&mut self, rsp0: u64) {
        self.rsp0 = rsp0;
    }
}

/// Build the I/O permission bitmap: deny all ports except VGA registers.
/// Bit = 0: allowed, Bit = 1: denied.
/// Allowed ranges:
///   0x1CE-0x1CF: Bochs VBE index/data registers
///   0x3B0-0x3DF: VGA registers (CRT, attribute, sequencer, graphics, etc.)
const fn build_iomap() -> [u8; 8191] {
    let mut map = [0xFFu8; 8191]; // deny all by default
    // Allow port range: clear bits for allowed ports
    // Port N → byte N/8, bit N%8
    let allowed: &[(u16, u16)] = &[
        (0x1CE, 0x1CF), // Bochs VBE
        (0x3B0, 0x3DF), // VGA
    ];
    let mut i = 0;
    while i < allowed.len() {
        let (start, end) = allowed[i];
        let mut port = start;
        while port <= end {
            map[(port / 8) as usize] &= !(1u8 << (port % 8));
            port += 1;
        }
        i += 1;
    }
    map
}

cpu_local! {
    pub static ref TSS: Tss = Tss {
        reserved0: 0,
        rsp0: 0,
        rsp1: 0,
        rsp2: 0,
        reserved1: 0,
        ist: [0; 7],
        reserved2: 0,
        reserved3: 0,
        iomap_offset: 104, // offsetof(Tss, iomap)
        iomap: build_iomap(),
        // According to Intel SDM, all bits of the last byte must be set to 1.
        iomap_last_byte: 0xff,
    };
}

pub unsafe fn init() {
    load_tr(SegmentSelector::from_raw(TSS_SEG));
}
