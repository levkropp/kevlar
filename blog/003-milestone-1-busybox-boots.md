# Milestone 1: BusyBox Boots on Kevlar

**Date:** 2026-03-08

---

Milestone 1 is done. Static musl BusyBox 1.31.1 boots on Kevlar in QEMU and runs an interactive shell. This is the proof-of-life moment for the kernel: a real userspace binary, compiled against musl libc, running on a kernel written in Rust under a permissive license.

```
# echo hello world
hello world
# ls /
bin                integration_tests  sys                var
dev                proc               tmp
etc                sbin               usr
```

`echo hello world`, `ls /`, `cat` -- they all work. It is not much, but it is real. A shell prompt backed by working process creation, filesystem traversal, TTY I/O, and signal handling, all running on Kevlar.

## What it took

### Syscall coverage: 35 to 79

Kerla shipped with around 35 meaningfully functional syscalls. Kevlar now implements 79, with 86 total dispatch entries (some share implementations or are thin wrappers). Getting BusyBox to an interactive shell required filling in gaps across the board: `ioctl` terminal ops, `fcntl` flags, `getdents64`, `clock_gettime`, signal delivery, and many others that a shell touches before it ever prints a prompt.

### Dependency upgrades

Every major Rust dependency was upgraded:

- **smoltcp** 0.7 to 0.12 -- this was the largest migration by far. The smoltcp API changed substantially across five major versions: socket sets were rearchitected, the PHY/device trait interfaces changed, and timestamp handling was overhauled. Not a drop-in update.
- **goblin** to 0.10
- **hashbrown** to 0.15
- **spin** to 0.10
- **buddy_system_allocator** to 0.11

### Target specification modernization

The x86_64 target JSON needed updates for current Rust nightly: switching to `rustc-abi`, using integer fields where the compiler now expects them, and adopting the `gnu-lld` linker flavor. The kind of thing that silently breaks when a project sits for a few years.

### Two critical boot bugs

These two consumed more debugging time than everything else combined.

**EFER.NXE not enabled.** Kerla's `boot.S` set the Long Mode Enable bit in the EFER MSR but never set No-Execute Enable. This was fine as long as no page table entries used the NX bit -- but the heap allocator maps pages with NX set. The result: an infinite loop of RESERVED_WRITE page faults immediately after heap initialization, with no useful error output. The fix was one line of assembly:

```asm
# Before: only Long Mode Enable
or eax, 0x0100

# After: Long Mode Enable + No-Execute Enable
or eax, 0x0900
```

One bit. Hours of staring at page fault dumps.

**compiler-builtins memcmp SSE crash.** The Rust `compiler-builtins` crate provides fallback implementations of `memcpy`, `memset`, and `memcmp`. The `memcmp` implementation uses `u128` comparisons, which LLVM lowers to SSE instructions. Kevlar runs with SSE disabled (the kernel does not save/restore SSE state on context switches). The result: an illegal instruction fault deep inside what looks like a simple memory comparison.

The fix was replacing the compiler-builtins memory functions with custom word-at-a-time implementations in `lang_items.rs` that operate on `usize` only -- no 128-bit operations, no SIMD, no surprises.

### Build pipeline

Kevlar now has a Docker-based initramfs build pipeline that produces a root filesystem with static musl BusyBox, dropbear SSH, and curl. Everything is statically linked against musl, so there are no dynamic linker dependencies to worry about yet (that comes at M2).

## The FreeBSD revelation

This was the most important architectural decision of M1, and it had nothing to do with code.

The original plan was to use OSv (BSD-3-Clause licensed) as the primary reference for syscall implementations. OSv is a unikernel that runs Linux binaries, so its syscall layer seemed like a natural model. And it is useful -- particularly for VFS and filesystem abstractions.

But FreeBSD turned out to be a far better reference for the core problem Kevlar is solving. FreeBSD's **linuxulator** (`sys/compat/linux/`) is a complete, battle-tested Linux syscall compatibility layer. FreeBSD developers have spent decades solving the exact problem we face: making Linux binaries run correctly on a non-Linux kernel. The linuxulator handles the subtle behavioral differences -- the edge cases in `ioctl` flag semantics, the Linux-specific `clone` flags, the precise signal delivery behavior that glibc and musl expect.

This matters for Kevlar's licensing story. FreeBSD is BSD-licensed. Re-implementing the linuxulator's approach to Linux compatibility in Rust is unambiguously clean from a copyright perspective: the BSD license explicitly permits this kind of use, and a full language rewrite is a transformation of high-level behavioral concepts, not a copy of code. There is no gray area here.

OSv remains valuable for its filesystem and I/O abstractions, but FreeBSD is now the primary reference for syscall semantics. Every future syscall implementation starts with reading the linuxulator source.

## Where Kevlar sits

Kevlar occupies a specific niche that did not previously exist. It is more Linux-native than FreeBSD's compatibility layer -- Kevlar *is* a Linux-ABI kernel, not a translation layer bolted onto a BSD kernel. Linux binaries are first-class citizens, not guests. But it is built on clean, permissively-licensed foundations: MIT/Apache-2.0, written in Rust, with the correctness guarantees that come from consulting FreeBSD's decades of POSIX expertise.

The kernel that Linux's userspace ecosystem deserves: permissively licensed, memory-safe, informed by the best available reference implementations.

## What's next

**M1.5: ARM64 support.** The platform-specific assembly in Kevlar is small. `boot.S` is roughly 250 lines of x86_64 assembly. Trap handling and user-space memory copy add another 175 lines or so. An ARM64 port is feasible and is the next target.

**M2: Dynamic linking.** BusyBox is statically linked. Real-world Linux binaries use `ld-linux.so`, which needs `pread64`, `futex`, `madvise`, and friends. M2 is about making dynamically linked musl binaries work, which opens the door to running unmodified distro packages.

Every new syscall will be implemented with FreeBSD's linuxulator as the primary reference for behavioral semantics. The pattern is established; now it scales.
