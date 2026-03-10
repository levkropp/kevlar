# Debugging 101

## Structured debug events

Kevlar emits structured JSONL debug events to the serial log. Enable them with the
`debug=` kernel parameter or the `KEVLAR_DEBUG` build-time environment variable:

```
make run CMDLINE="debug=all"         # All event categories
make run CMDLINE="debug=syscall"     # Syscall trace only
make run CMDLINE="debug=fault"       # Page fault events
make run CMDLINE="debug=signal"      # Signal delivery events
```

Available categories: `all`, `syscall`, `signal`, `fault`, `process`, `canary`,
`memory`, `panic`.

Events are prefixed with `DBG ` in the raw serial output for grep-ability:

```
DBG {"event":"syscall","nr":1,"name":"write","pid":2,"ret":14}
```

## make debug

`make debug` boots with GDB attached and debug events written to `debug.jsonl`:

```
make debug          # Boot with GDB on localhost:7789, events → debug.jsonl
```

In a second terminal:

```
gdb kernel.elf
(gdb) target remote localhost:7789
(gdb) continue
```

## Crash analysis

When the kernel panics or triple-faults:

```
make analyze-crash   # Analyzes kevlar.dump
make analyze-log     # Analyzes serial log for patterns
```

The crash analyzer (`tools/crash-analyzer/analyzer.py`) decodes backtraces,
identifies common failure patterns (null deref, canary corruption, missing syscalls),
and summarizes the crash context.

## Verbose logging

See [Logging](../logging.md) for controlling log verbosity with `LOG=`.

## QEMU exits without a kernel panic message

Use QEMU's interrupt logging to find the cause:

```
qemu-system-x86_64 -d int,cpu_reset 2>&1 | head -50
```

Common causes:
- Triple fault from a bad IDT entry or stack overflow
- `#GP` from executing at a non-canonical address (stack corruption)
- `#PF` with `RESERVED_WRITE` set (NX bit on kernel heap pages — now fixed)

## Debugging virtio devices

Build QEMU from source and enable its device-specific `DEBUG` macros. For
virtio-net: `hw/net/virtio-net.c`. For virtio-blk: `hw/block/virtio-blk.c`.

QEMU's tracing feature also helps:

```
qemu-system-x86_64 -trace "virtio*" ...
```
