---
name: crash_diagnostics
description: Crash analysis tooling — per-process syscall trace, CrashReport debug event, crash-report.py tool
type: project
---

Per-process crash diagnostics implemented (2026-03-15):

- **Per-process syscall trace**: Every process has a 32-entry lock-free ring buffer recording the last 32 syscalls (nr, result, first 2 args). Unconditional recording, ~5ns per syscall.
- **CrashReport debug event**: When a process crashes (SIGSEGV, etc.), the kernel emits a structured JSONL event with: pid, signal, cmdline, fault_addr, ip, fsbase, registers (placeholder 0s until per-CPU stash is implemented), last 32 syscalls, and VMA map (up to 64 entries).
- **crash-report.py**: Python tool that parses QEMU serial output and generates human-readable crash reports with: disassembly around crash IP (via objdump), symbol resolution (via addr2line/nm), syscall history, and memory map.
- **SIGSEGV logging**: All SIGSEGV paths in page_fault.rs now use `warn!` (always visible) instead of `debug_warn!` (debug-only).

**Why:** Manual debugging of userspace crashes was extremely slow — required piping output through grep, objdump, etc. The crash report gives full context in one shot.

**How to apply:** When diagnosing userspace crashes, run with `debug=fault,process` and pipe through `tools/crash-report.py`. The JSONL format is also consumable by the MCP debug server.

**Known limitation:** Register values are currently 0 in crash reports (per-CPU register stash not yet implemented). The crash IP and fault address are available.
