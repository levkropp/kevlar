#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
"""Generate a Linux/x86 Boot Protocol bzImage for Kevlar.

Usage: gen_setup.py <kernel_bin> <output_img> <kernel_elf>

Prepends a minimal 1024-byte (2-sector) Linux/x86 Boot Protocol v2.12 setup
block to <kernel_bin> and writes the result to <output_img>.

<kernel_elf> is used to read the ELF entry point, which becomes code32_start
(the address QEMU jumps to in 32-bit protected mode after loading the kernel).

Any bzImage-aware bootloader (QEMU, GRUB2, SYSLINUX, UEFI Linux EFI stub) will:
  1. Read the setup sector for metadata.
  2. Load the kernel body (file bytes after the setup) at code32_start.
  3. Build a struct boot_params in memory.
  4. Enter at code32_start in 32-bit protected mode with
     ESI = physical address of boot_params.

Our boot.S entry detects the Linux boot protocol and sets
EAX = LINUXBOOT_MAGIC / EBX = boot_params, which bootinfo::parse() handles.

All offsets below are from Documentation/x86/boot.rst.
"""

import struct
import sys


# Setup header fields
SETUP_SECTS   = 1         # 1 → two 512-byte setup sectors (1024 bytes total)
                          # Required: the extended header starts at 0x200 which
                          # is the first byte of sector 1.
BOOT_FLAG     = 0xAA55    # offset 0x1FE — detected by bootloaders
JUMP_INSTR    = 0x4EEB    # EB 4E: short jump past the header (to 0x250)
HDRS_MAGIC    = 0x53726448  # "HdrS" at offset 0x202
PROTOCOL_VER  = 0x020C    # Boot protocol 2.12
LOADFLAGS     = 0x01      # LOADED_HIGH: kernel loads at code32_start
KERNEL_BASE   = 0x100000  # Physical address where kernel is loaded AND entered
INITRD_MAX    = 0x7FFFFFFF
KERNEL_ALIGN  = 0x200000  # 2 MB
MIN_ALIGN     = 21        # 2^21 = 2 MB
XLOADFLAGS    = 0x0000    # No XLF_KERNEL_64: bootloader must use 32-bit entry at
                          # code32_start.  We do not implement the Linux x86_64
                          # 64-bit entry convention (startup_64 at code32_start+0x200).
CMDLINE_SIZE  = 0x7FF
INIT_SIZE     = 0x4000000  # 64 MB — generous upper bound


def make_setup_sector() -> bytes:
    # Two sectors = 1024 bytes: sector 0 (0x000-0x1FF) holds the legacy fields
    # and boot_flag; sector 1 (0x200-0x3FF) holds the extended setup header.
    sector = bytearray(1024)

    # Offset 0x000: real-mode entry code stub (never executed by modern
    # bootloaders, but must be a valid instruction stream).  EB FE = JMP -2
    # (infinite loop) is safe and unambiguous.
    sector[0x000] = 0xEB
    sector[0x001] = 0xFE  # infinite loop — never runs

    # Offset 0x1F1: setup_sects
    sector[0x1F1] = SETUP_SECTS

    # Offset 0x1FE: boot_flag = 0xAA55 (LE)
    struct.pack_into('<H', sector, 0x1FE, BOOT_FLAG)

    # Offset 0x200: two-byte JMP to skip past the header fields.
    # EB 4E = JMP SHORT +0x4E, landing at 0x202 + 0x4E = 0x250.
    struct.pack_into('<H', sector, 0x200, JUMP_INSTR)

    # Offset 0x202: "HdrS" magic
    struct.pack_into('<I', sector, 0x202, HDRS_MAGIC)

    # Offset 0x206: protocol version
    struct.pack_into('<H', sector, 0x206, PROTOCOL_VER)

    # Offset 0x211: loadflags (LOADED_HIGH)
    sector[0x211] = LOADFLAGS

    # Offset 0x214: code32_start — load address and 32-bit entry point.
    struct.pack_into('<I', sector, 0x214, KERNEL_BASE)

    # Offset 0x22C: initrd_addr_max
    struct.pack_into('<I', sector, 0x22C, INITRD_MAX)

    # Offset 0x230: kernel_alignment
    struct.pack_into('<I', sector, 0x230, KERNEL_ALIGN)

    # Offset 0x234: relocatable_kernel = 1
    sector[0x234] = 1

    # Offset 0x235: min_alignment (power of two)
    sector[0x235] = MIN_ALIGN

    # Offset 0x236: xloadflags
    struct.pack_into('<H', sector, 0x236, XLOADFLAGS)

    # Offset 0x238: cmdline_size
    struct.pack_into('<I', sector, 0x238, CMDLINE_SIZE)

    # Offset 0x258: pref_address (u64) — preferred load address for the body
    struct.pack_into('<Q', sector, 0x258, KERNEL_BASE)

    # Offset 0x260: init_size — linear memory needed during init
    struct.pack_into('<I', sector, 0x260, INIT_SIZE)

    return bytes(sector)


def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <kernel_bin> <output_img>", file=sys.stderr)
        sys.exit(1)

    kernel_bin_path = sys.argv[1]
    output_img_path = sys.argv[2]

    setup = make_setup_sector()
    with open(kernel_bin_path, 'rb') as f:
        kernel = f.read()

    with open(output_img_path, 'wb') as f:
        f.write(setup)
        f.write(kernel)

    print(f"gen_setup.py: wrote {len(setup) + len(kernel)} bytes to {output_img_path} "
          f"({len(setup)}-byte setup + {len(kernel)} bytes kernel at 0x{KERNEL_BASE:x})")


if __name__ == '__main__':
    main()
