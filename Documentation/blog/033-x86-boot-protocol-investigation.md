# x86 Linux Boot Protocol: A QEMU 10.x Investigation

*2026-03-10*

## Background

When we added the ARM64 Linux Image header to Kevlar (milestone 1.5), it made QEMU
recognise our kernel as a proper ARM64 Linux kernel and pass `x0=DTB` correctly.
Before that fix, QEMU would load our ELF directly but leave `x0=0`, meaning we had
no device-tree and the kernel would fail to find memory.

The natural question: should we do the same for x86?  QEMU's `-kernel` path for x86
also has a "native" Linux boot protocol — the **bzImage / Linux/x86 Boot Protocol** —
where QEMU recognises a setup sector (0xAA55 at file offset 0x1FE, "HdrS" magic at
0x202) and uses SeaBIOS's `linuxboot.rom` option ROM to boot the kernel.  Without it,
QEMU uses an internal multiboot ELF loader that has historically required
`e_machine = EM_386 (3)` even for a 64-bit kernel.

In theory the bzImage path is more correct and future-proof: any bootloader (GRUB2,
SYSLINUX, UEFI Linux EFI stub) can use it, and it gives us the full `struct
boot_params` / E820 memory map instead of multiboot.

So we implemented it: `platform/x64/gen_setup.py` builds a 1024-byte setup sector and
prepends it to the flat kernel binary, producing `kevlar.x64.img`.

Then things got interesting.

## The Triple Fault

When we first ran with the bzImage (`-kernel kevlar.x64.img`) the VM triple-faulted
immediately.  No output.  Time to debug.

### Adding COM1 debug markers

The x86 boot path is notoriously hard to debug with GDB alone because the CPU starts
in whatever mode the bootloader left it in.  We added a `COM1_PUTC` macro that polls
the UART LSR before writing (works in both 32-bit and 64-bit mode from ring 0):

```asm
.macro COM1_PUTC ch
        mov dx, 0x3fd      // COM1 LSR — must use DX, port > 255
9997:   in  al, dx
        test al, 0x20      // TX empty?
        jz  9997b
        mov al, \ch
        mov dx, 0x3f8      // COM1 TX
        out dx, al
.endm
```

Two subtle pitfalls discovered during this:

1. **COM1 port numbers (0x3F8, 0x3FD) are > 255**, so they cannot be used as
   immediate operands in `in`/`out`.  Must load into DX first.  The assembler gives
   "invalid operand for instruction" otherwise.

2. **`test al, 0x20` clobbers EFLAGS (ZF)**.  If you place a `COM1_PUTC` between a
   `test eax, 0x0100` (checking EFER.LME) and the `jz boot32` that follows, the ZF
   is clobbered and the branch always falls through.  Move the marker *after* the
   branch.

We placed markers 'A'–'H' at key points in the boot path.

### Root cause: XLF_KERNEL_64

Markers 'A' through 'D' printed.  Then silence.  After 'D' (`lgdt` + `retf` into
protected mode) the CPU stopped responding.  GDB confirmed the machine was executing
garbage.

Looking at what happens after `retf` into protected mode: we land in `protected_mode:`
and call `lgdt [boot_gdtr]` again, then `retf` into `enable_long_mode:`.  All fine.

So the crash was actually before any of our code ran.  The kernel was never reached.

Time to read the SeaBIOS linuxboot.rom source.  The relevant field:

```
Offset 0x236: xloadflags
  Bit 0 (XLF_KERNEL_64): If set, the kernel supports 64-bit entry at
  code32_start + 0x200 (i.e. startup_64).
```

Our original `gen_setup.py` had set `XLOADFLAGS = 0x0001` — XLF_KERNEL_64 enabled.
This tells linuxboot.rom to jump to `code32_start + 0x200 = 0x100000 + 0x200 =
0x100200` instead of `code32_start = 0x100000`.

What's at 0x100200 in our kernel?  That's offset 0x200 into the flat binary, which is
the **middle of the multiboot2 header** — garbage as x86 machine code.  Instant crash.

Fix: `XLOADFLAGS = 0x0000`.  We do not implement the Linux x86_64 64-bit entry
convention (`startup_64` at `code32_start+0x200`).

### Still not booting

After fixing `XLOADFLAGS=0`, the triple fault was gone, but the kernel *still* didn't
boot.  GDB hardware breakpoint at 0x100000 was set but never hit — even after 4+
minutes.

We confirmed via GDB:
- The kernel binary IS mapped at 0x100000 (bytes match `jmp boot_main`)
- The `struct boot_params` area at 0x90000 is all zeros (linuxboot.rom hasn't run)
- The CPU was stuck executing zeros in the BIOS area (0xFC38, 0xEC38 — SeaBIOS
  internal addresses)

The serial output showed "Booting from ROM.." — meaning SeaBIOS *did* invoke
`linuxboot_dma.bin` — but the ROM failed silently before ever jumping to `code32_start`.

We spent considerable time verifying the setup header fields, checking the linuxboot
source, and reading QEMU fw_cfg documentation.  The ROM was loading our kernel into
memory but failing at the final jump.

This appears to be a **QEMU 10.x regression** in the x86 `-kernel` bzImage path.  The
linuxboot.rom mechanism is fragile: it relies on fw_cfg DMA, firmware tables, and
SeaBIOS internals, and something in the QEMU 10.x / current Arch Linux QEMU build is
broken for this code path.

## The Pragmatic Fix

Rather than debugging SeaBIOS's linuxboot.rom internals, we chose the pragmatic
approach: continue using QEMU's **internal multiboot ELF loader** (which works
reliably), but produce the **bzImage as a separate artifact** for real hardware.

The multiboot loader requires `e_machine = EM_386 (3)` — QEMU's `multiboot.c`
rejects `EM_X86_64 (62)` with "Cannot load x86-64 image, give a 32bit one." — even
though our 64-bit kernel boots just fine after the multiboot handoff.

`tools/run-qemu.py` now patches a temporary copy of the ELF:

```python
if args.arch == "x64":
    with open(kernel_path_arg, 'rb') as f:
        elf_data = bytearray(f.read())
    elf_data[18] = 0x03  # e_machine low byte: EM_386
    elf_data[19] = 0x00  # e_machine high byte
    tmp_fd, tmp_elf_path = tempfile.mkstemp(suffix=".elf")
    os.write(tmp_fd, elf_data)
    os.close(tmp_fd)
    kernel_path_arg = tmp_elf_path
```

The `kevlar.x64.img` bzImage is still built by the Makefile and works correctly with
GRUB2 on real hardware.  The Makefile now passes `$(kernel_elf)` (not `$(kernel_img)`)
to `run-qemu.py` for x64, with the EM_386 patching handled inside the script.

## Other fixes from this investigation

**`cmd_line_ptr = 0` UB in `bootinfo.rs`**: `parse_linux_boot_params` was calling
`core::slice::from_raw_parts(setup_header.cmd_line_ptr as *const u8, ...)` without
checking if `cmd_line_ptr == 0`.  If no `-append` is given, QEMU leaves this field
zero, creating a null-pointer slice — undefined behaviour.  Fixed with a null check.

**`XLOADFLAGS` documentation**: Updated `gen_setup.py` with an explicit comment
explaining why `XLOADFLAGS = 0x0000` is correct for Kevlar.  We do not implement the
`startup_64` entry point at `code32_start+0x200`.

## Future work: proper bzImage boot in QEMU

The right long-term fix is to implement the `startup_64` entry convention that
`XLF_KERNEL_64` requires, so that the bzImage path works end-to-end in QEMU.  That
means adding a 64-bit entry stub at exactly `code32_start + 0x200` that:

1. Receives `RSI = struct boot_params *` (64-bit pointer)
2. Checks the `boot_params` magic to distinguish from our LINUXBOOT_MAGIC path
3. Jumps to `boot_main` with the appropriate register setup

This would make Kevlar a fully drop-in replacement for Linux in QEMU's `-kernel` path
without any e_machine trickery, and would also work in Firecracker (which uses the
64-bit entry convention).  Tracking issue: TODO.

## Summary

| Symptom | Root cause | Fix |
|---------|-----------|-----|
| Triple fault at boot | `XLF_KERNEL_64=1` → linuxboot.rom jumps to `code32_start+0x200` (garbage) | `XLOADFLAGS=0x0000` in gen_setup.py |
| Kernel never reached after XLF fix | QEMU 10.x linuxboot.rom broken on this system | Restore EM_386 ELF patching in run-qemu.py |
| `cmd_line_ptr=0` UB | No null check before `from_raw_parts` | Add null guard in parse_linux_boot_params |
| COM1_PUTC build error | Ports > 255 can't be immediate operands | Use DX register for COM1 port addresses |
| EFLAGS clobbered | `test al, 0x20` inside COM1_PUTC between `test eax` and `jz` | Move debug marker after the branch |
