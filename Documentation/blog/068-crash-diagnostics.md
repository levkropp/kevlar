# M10 Phase 7b: Crash Diagnostics + sync Stub

Debugging the `apk.static` SIGSEGV took hours of manual `grep`, `objdump`,
and re-running QEMU with different `debug=` flags. The kernel had the data
— fault address, instruction pointer, memory map — but only printed a
one-line warning. All the rich context was lost by the time the process
exited.

## Per-process syscall trace

Every process now records its last 32 syscalls in a lock-free ring buffer.
The buffer uses `AtomicCell` entries and an `AtomicU32` write index — one
relaxed `fetch_add` plus one atomic store per syscall, ~5ns overhead.
Recording is unconditional for all processes, not just PID 1.

```rust
pub struct SyscallTrace {
    entries: [AtomicCell<SyscallTraceEntry>; PROC_TRACE_LEN],
    write_idx: AtomicU32,
}
```

On crash, `dump_trace()` returns the entries in chronological order. This
replaced the global PID-1-only trace buffer for crash diagnostics.

## CrashReport debug event

When a process dies by fatal signal, the kernel now emits a structured
`CrashReport` JSONL event containing:

- PID, signal name, command line
- Fault address and instruction pointer
- FS base (TLS pointer)
- Last 32 syscalls with resolved names
- Up to 64 VMAs from the process memory map

The event is emitted from three places: the null-pointer, invalid-address,
and no-VMA paths in the page fault handler, plus the general
`exit_by_signal` catch-all. The VMA collection uses `is_locked()` to avoid
deadlock if the crash was caused by a VM lock issue.

```
DBG {"type":"crash_report","pid":22,"signal":11,"signal_name":"SIGSEGV",
     "cmdline":"apk.static --root /mnt info","fault_addr":0x0,"ip":0x420000,
     "fsbase":0x88f5f8,"regs":{...},
     "syscalls":[{"nr":257,"name":"openat","result":6,"a0":0x3,"a1":0x742266},
                 {"nr":9,"name":"mmap","result":2465792,"a0":0x0,"a1":0x2004c}],
     "vmas":[{"start":0x400000,"end":0x89328c,"type":"file"},
             {"start":0x9fffdf000,"end":0xa00000000,"type":"anon"},...]}
```

## crash-report.py

A Python tool that parses QEMU serial output and generates human-readable
crash reports:

```
========================================================================
  CRASH REPORT: PID 22 (apk.static --root /mnt info) killed by SIGSEGV
========================================================================

  Fault address: 0x0
  Instruction:   0x420000
  FS base:       0x88f5f8

  Disassembly around 0x420000:
      41ffe4:  64 48 8b 04 25 28 00  mov    %fs:0x28,%rax
  >>> 420000:  48 85 ff              test   %rdi,%rdi

  Last 32 syscalls (oldest first):
    [30] openat(0x3, 0x742266) -> 6
    [31] mmap(0x0, 0x2004c) -> 0x25a000

  Memory map (39 VMAs):
    0x000000400000-0x00000089328c (   4M) file
    0x0009fffdf000-0x000a00000000 ( 132K) anon
    ...
```

Auto-disassembly via `objdump`, symbol resolution via `addr2line`/`nm`,
and `--json` mode for automation.

Usage:
```
python3 tools/run-qemu.py --disk build/alpine-disk.img \
  --append-cmdline "debug=fault,process" kevlar.x64.elf 2>&1 \
  | python3 tools/crash-report.py --binary /tmp/apk.static
```

## SIGSEGV always-on logging

All four SIGSEGV paths in the page fault handler now use `warn!` instead
of `debug_warn!`. Fatal signal delivery is rare — always worth logging.
Each path prints the fault address, PID, instruction pointer, and reason:

```
SIGSEGV: null pointer access (pid=22, ip=0x420000, fsbase=0x88f5f8)
SIGSEGV: no VMA for address 0xdeadbeef (pid=5, ip=0x401234, reason=CAUSED_BY_WRITE)
```

## sync(2) stub

`poweroff -f` calls `sync()` before issuing `reboot(2)`. Syscall 162 on
x86_64 (81 on arm64) was unimplemented, producing a harmless but confusing
warning on every shutdown. Now returns 0 — correct since ext2 writes are
synchronous (no write-back cache).

## QEMU exit hint

`run-qemu.py` now prints `Press Ctrl-A X to exit QEMU` on interactive
sessions. With `-serial mon:stdio`, Ctrl-C is captured as serial input
to the guest. The QEMU escape sequence is Ctrl-A then X.

## Per-CPU register stash

The interrupt handler now stashes all GP registers + RIP + RSP + RFLAGS
to a per-CPU static array before dispatching the page fault handler.
This costs ~10ns per page fault (19 relaxed atomic stores) — negligible
on 2900ns demand-page faults. The crash report reads the stash and
includes real register values.

## chroot(2) + sync(2)

`chroot(2)` implemented: changes the process's root directory via
`RootFs::chroot()`. This enables `chroot /mnt /sbin/apk info` which
successfully lists Alpine packages from the ext2 rootfs.

`sync(2)` stubbed (returns 0) — our ext2 writes are synchronous, so
sync is a no-op. Eliminates the "unimplemented syscall 162" warning on
`poweroff -f`.

## QEMU exit hint

`run-qemu.py` prints "Press Ctrl-A X to exit QEMU" on interactive
sessions (TTY-only). With `-serial mon:stdio`, Ctrl-C becomes serial
input — the QEMU escape is Ctrl-A then X.

## Files changed

| File | Change |
|------|--------|
| `kernel/process/process.rs` | Per-process SyscallTrace ring buffer, CrashReport emission in exit_by_signal |
| `kernel/debug/event.rs` | CrashReport variant + JSONL serialization |
| `kernel/mm/page_fault.rs` | emit_crash_and_exit helper, always-on SIGSEGV logging |
| `kernel/syscalls/mod.rs` | Unconditional per-process trace recording, sync(2) stub |
| `tools/crash-report.py` | New: crash report parser with auto-disassembly |
| `kernel/syscalls/chroot.rs` | New: chroot(2) syscall |
| `kernel/fs/mount.rs` | RootFs::chroot() method |
| `kernel/syscalls/mod.rs` | sync(2) stub, chroot dispatch |
| `platform/crash_regs.rs` | New: per-CPU register stash |
| `platform/x64/interrupt.rs` | Stash registers before page fault dispatch |
| `tools/run-qemu.py` | Ctrl-A X exit hint for interactive sessions |
