#!/usr/bin/env python3
"""
Kevlar Kernel Debug MCP Server

An MCP (Model Context Protocol) server that exposes POSIX kernel debugging
tools for use by LLMs and Claude Code. Designed for debugging stack canary
failures, syscall errors, signal delivery issues, and general kernel faults.

Usage:
    # Start QEMU with debug events enabled:
    make run KEVLAR_DEBUG=all

    # In another terminal, start the MCP server:
    python3 tools/mcp-debug-server/server.py \\
        --debug-log debug.jsonl \\
        --elf kevlar.x64.elf \\
        --symbols kevlar.x64.symbols

    # Or configure in Claude Code's MCP settings (see README).

Architecture:
    The server has three data sources:
    1. Debug event stream (JSONL from kernel serial output)
    2. GDB/MI bridge (live kernel state via QEMU GDB stub)
    3. Symbol resolver (offline symbol lookup from .symbols file)
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Optional

from mcp.server.fastmcp import FastMCP

from event_stream import DebugEventStream
from gdb_bridge import GDBBridge, SymbolResolver

# ── Server setup ──

mcp = FastMCP(
    "kevlar-debug",
    version="0.1.0",
    description="POSIX kernel debug tooling for Kevlar (and compatible kernels)",
)

# Global state — initialized in main().
stream: Optional[DebugEventStream] = None
gdb: Optional[GDBBridge] = None
symbols: Optional[SymbolResolver] = None


# ── Session & Summary Tools ──


@mcp.tool()
def debug_summary() -> dict:
    """Get an executive summary of the current debug session.

    Returns aggregate counts of syscalls, errors, signals, faults,
    canary corruptions, and panics. Start here to understand what's
    happening in the kernel.
    """
    if stream is None:
        return {"error": "No debug event stream configured"}
    return stream.get_summary()


@mcp.tool()
def get_recent_events(
    last_n: int = 50,
    pid: Optional[int] = None,
    event_type: Optional[str] = None,
) -> list[dict]:
    """Get recent debug events, optionally filtered by PID or event type.

    Event types: syscall_entry, syscall_exit, canary_check, page_fault,
    signal, user_fault, process_exit, process_exec, process_fork,
    panic, unimplemented_syscall, usercopy, usercopy_fault, signal_stack_write
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.query(pid=pid, event_type=event_type, last_n=last_n)


# ── Syscall Debugging Tools ──


@mcp.tool()
def get_syscall_trace(
    pid: Optional[int] = None,
    last_n: int = 50,
    name_filter: Optional[str] = None,
) -> list[dict]:
    """Get recent syscall trace entries (entry + exit pairs).

    Like strace but structured. Each entry shows:
    - Syscall name, number, and arguments
    - Return value or errno on exit
    - PID of the calling process

    Use name_filter to focus on specific syscalls (e.g. "open", "mmap").
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_syscall_trace(pid=pid, last_n=last_n, name_filter=name_filter)


@mcp.tool()
def get_failed_syscalls(
    errno: Optional[str] = None, last_n: int = 100
) -> list[dict]:
    """Get syscalls that returned errors, optionally filtered by errno.

    Common errnos: ENOENT, EFAULT, EINVAL, ENOSYS, ENOMEM, EBADF, EPERM.
    Useful for finding systematic failures or permission issues.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_failed_syscalls(errno=errno, last_n=last_n)


@mcp.tool()
def get_syscall_error_summary() -> dict:
    """Aggregate syscall errors by syscall name and errno.

    Returns: {syscall_name: {errno: count}}
    Helps identify patterns like all open() calls returning ENOENT,
    or repeated EFAULT from read()/write() indicating bad pointers.
    """
    if stream is None:
        return {"error": "No debug event stream configured"}
    return stream.get_syscall_error_summary()


@mcp.tool()
def get_unimplemented_syscalls() -> list[dict]:
    """Get all syscalls the kernel doesn't implement yet (ENOSYS).

    Shows which syscalls userspace programs are trying to use that the
    kernel doesn't support. Essential for knowing what to implement next.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.query(event_type="unimplemented_syscall", last_n=100)


# ── Stack Canary / Corruption Tools ──


@mcp.tool()
def get_canary_corruptions() -> list[dict]:
    """Get all detected stack canary corruptions.

    Each entry shows:
    - PID and fsbase (TLS base address)
    - Expected vs actual canary value
    - Which syscall was executing when corruption was detected
    - Whether it was detected before or after the syscall

    A corrupted canary indicates a stack buffer overflow in userspace.
    The syscall name tells you which operation may have written past
    a buffer boundary.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_canary_corruptions()


# ── Usercopy Debugging Tools ──


@mcp.tool()
def get_usercopy_faults() -> list[dict]:
    """Get all usercopy fault events (page faults during copy_to/from_user).

    Each entry shows:
    - Faulting address and instruction pointer
    - Which usercopy phase (leading_bytes, bulk_qwords, trailing_bytes, strncpy, memset)
    - Context tag identifying which kernel operation was performing the copy
    - Register state at fault time (dst, src, remaining count)

    The 'label' field is critical for diagnosing which copy path failed:
    - trailing_bytes = copy was ≥8 bytes, fault in the len%8 remainder
    - bulk_qwords = fault during the main rep movsq loop
    - leading_bytes = fault during alignment of first bytes
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_usercopy_faults()


@mcp.tool()
def get_usercopy_trace(
    pid: Optional[int] = None, last_n: int = 100
) -> list[dict]:
    """Get usercopy trace events (every copy_to/from_user call).

    High volume — only emitted when USERCOPY debug filter is enabled.
    Each entry shows: direction (to_user/from_user), user address,
    length, and context tag (e.g. "ioctl:TCGETS", "sys_uname").

    Use this to see all userspace memory accesses around a fault.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_usercopy_trace(pid=pid, last_n=last_n)


@mcp.tool()
def get_usercopy_trace_dumps(last_n: int = 10) -> list[dict]:
    """Get assembly-level usercopy trace dumps.

    These are emitted automatically when canary corruption or a usercopy fault
    is detected. Each dump contains the last 32 copy_to_user/copy_from_user
    calls with their ACTUAL CPU register values at entry:
    - dst: destination pointer (rdi)
    - src: source pointer (rsi)
    - len: byte count (rdx) — THIS reveals the wrong-length copy
    - ret: return address — identifies which Rust function called copy_to_user

    The 'trigger' field shows why the dump was taken (canary_corruption, usercopy_fault).
    Look for entries where 'len' doesn't match the expected size for the operation.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.query(event_type="ucopy_trace_dump", last_n=last_n)


@mcp.tool()
def get_signal_stack_writes(
    pid: Optional[int] = None, last_n: int = 50
) -> list[dict]:
    """Get signal stack setup trace events.

    Shows each individual write to the user stack during signal delivery:
    - What was written (trampoline, return_addr, siginfo, etc.)
    - User address and length of each write
    - RSP before and after the write

    Critical for debugging signal delivery faults — shows if the user
    stack pointer was valid and how it was manipulated.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_signal_stack_writes(pid=pid, last_n=last_n)


# ── Signal & Fault Tools ──


@mcp.tool()
def get_signal_history(
    pid: Optional[int] = None, last_n: int = 50
) -> list[dict]:
    """Get signal delivery history.

    Each entry shows: signal number/name, action taken (ignore/terminate/
    stop/continue/handler), handler address if applicable, and target PID.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_signal_history(pid=pid, last_n=last_n)


@mcp.tool()
def get_fault_history(last_n: int = 50) -> list[dict]:
    """Get page fault and CPU exception history.

    Page faults show: virtual address, instruction pointer, whether the
    fault was resolved (demand paging) or fatal (SIGSEGV), and VMA info.

    User faults show: exception type (GPF, SIGFPE, etc), instruction
    pointer, and which signal was delivered.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_fault_history(last_n=last_n)


# ── Process Tools ──


@mcp.tool()
def get_process_events(last_n: int = 50) -> list[dict]:
    """Get process lifecycle events (fork, exec, exit).

    Shows the process tree evolution: which PIDs were created (fork),
    what programs they ran (exec), and how they terminated (exit status
    or signal).
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_process_events(last_n=last_n)


# ── Panic Analysis ──


@mcp.tool()
def get_panics() -> list[dict]:
    """Get all kernel panic events with structured backtraces.

    Each panic includes: the panic message, and a backtrace with
    resolved symbol names and offsets. Use resolve_address() to get
    more detail on specific addresses.
    """
    if stream is None:
        return [{"error": "No debug event stream configured"}]
    return stream.get_panics()


# ── Symbol Resolution ──


@mcp.tool()
def resolve_address(address: int) -> dict:
    """Resolve a virtual address to a kernel symbol name + offset.

    Works offline using the .symbols file. No GDB connection needed.
    Useful for interpreting addresses from backtraces, fault IPs, etc.

    Accepts addresses as integers (e.g. 0xffffffff80100000).
    """
    if symbols is None:
        return {"error": "No symbols file loaded"}
    return symbols.resolve(address)


# ── Live GDB Tools (require QEMU with -gdb) ──


@mcp.tool()
def read_kernel_memory(
    address: int, length: int = 64, fmt: str = "hex"
) -> dict:
    """Read kernel memory at the given address (requires GDB connection).

    Format options: "hex" (raw hex), "ascii" (string), "u64_array" (parsed u64s).
    Useful for inspecting data structures, stack frames, or page contents.
    """
    if gdb is None or not gdb.connected:
        return {"error": "GDB not connected. Start QEMU with --gdb flag."}
    result = gdb.read_memory(address, length, fmt)
    return result or {"error": f"Failed to read memory at {address:#x}"}


@mcp.tool()
def get_cpu_registers() -> dict:
    """Get current CPU register state (requires GDB, kernel must be stopped).

    Returns all general-purpose registers. The kernel must be paused
    (e.g. hit a breakpoint or manually interrupted).
    """
    if gdb is None or not gdb.connected:
        return {"error": "GDB not connected"}
    result = gdb.get_registers()
    return result or {"error": "Failed to read registers"}


@mcp.tool()
def get_gdb_backtrace() -> list[dict]:
    """Get the current kernel backtrace via GDB (kernel must be stopped).

    More detailed than the in-kernel backtrace — includes file/line info
    if debug symbols are available.
    """
    if gdb is None or not gdb.connected:
        return [{"error": "GDB not connected"}]
    return gdb.get_backtrace()


@mcp.tool()
def gdb_evaluate(expression: str) -> dict:
    """Evaluate a GDB expression (requires GDB connection).

    Can read global variables, dereference pointers, cast types, etc.
    Example: "PROCESSES", "*(int*)0xffffffff80200000"
    """
    if gdb is None or not gdb.connected:
        return {"error": "GDB not connected"}
    result = gdb.evaluate(expression)
    return {"expression": expression, "value": result}


# ── Crash Dump Analysis ──


@mcp.tool()
def analyze_crash_dump(dump_path: str) -> dict:
    """Analyze a kevlar.dump crash dump file.

    Parses the boot2dump crash dump format and extracts:
    - Kernel log (last messages before crash)
    - Any structured debug events in the log
    - Detected bug patterns (canary corruption, null deref, etc.)

    The dump file is typically saved by boot2dump after a kernel panic.
    """
    p = Path(dump_path)
    if not p.exists():
        return {"error": f"Dump file not found: {dump_path}"}

    try:
        data = p.read_bytes()

        # Parse the KernelDump struct: magic(4) + len(4) + log(4096)
        if len(data) < 8:
            return {"error": "Dump too small"}

        magic = int.from_bytes(data[0:4], "little")
        if magic != 0xDEADBEEE:
            return {"error": f"Bad magic: {magic:#x} (expected 0xdeadbeee)"}

        log_len = int.from_bytes(data[4:8], "little")
        log_data = data[8 : 8 + min(log_len, 4096)]
        log_text = log_data.decode("utf-8", errors="replace")

        # Extract structured events from the log.
        events = []
        log_lines = []
        for line in log_text.splitlines():
            if line.startswith("DBG "):
                try:
                    events.append(json.loads(line[4:]))
                except json.JSONDecodeError:
                    log_lines.append(line)
            else:
                log_lines.append(line)

        # Detect known patterns.
        patterns = _detect_crash_patterns(log_text, events)

        return {
            "magic": f"{magic:#x}",
            "log_length": log_len,
            "log_lines": log_lines[-50:],  # Last 50 lines
            "structured_events": events,
            "detected_patterns": patterns,
        }
    except Exception as e:
        return {"error": f"Failed to parse dump: {e}"}


@mcp.tool()
def analyze_serial_log(log_path: str) -> dict:
    """Analyze a raw QEMU serial log file.

    Separates structured debug events (DBG lines) from regular kernel
    output, computes statistics, and detects patterns.
    """
    p = Path(log_path)
    if not p.exists():
        return {"error": f"Log file not found: {log_path}"}

    text = p.read_text(errors="replace")
    events = []
    log_lines = []
    for line in text.splitlines():
        if line.startswith("DBG "):
            try:
                events.append(json.loads(line[4:]))
            except json.JSONDecodeError:
                log_lines.append(line)
        else:
            log_lines.append(line)

    # Build summary.
    type_counts = {}
    for e in events:
        t = e.get("type", "unknown")
        type_counts[t] = type_counts.get(t, 0) + 1

    syscall_errors = {}
    for e in events:
        if e.get("type") == "syscall_exit" and "errno" in e:
            name = e.get("name", "?")
            errno = e["errno"]
            syscall_errors.setdefault(name, {})
            syscall_errors[name][errno] = syscall_errors[name].get(errno, 0) + 1

    patterns = _detect_crash_patterns(text, events)

    return {
        "total_lines": len(text.splitlines()),
        "total_events": len(events),
        "event_type_counts": type_counts,
        "syscall_error_summary": syscall_errors,
        "detected_patterns": patterns,
        "last_log_lines": log_lines[-30:],
        "panics": [e for e in events if e.get("type") == "panic"],
        "canary_corruptions": [
            e for e in events if e.get("type") == "canary_check" and e.get("corrupted")
        ],
    }


def _detect_crash_patterns(log_text: str, events: list[dict]) -> list[dict]:
    """Detect known bug patterns in crash data."""
    patterns = []

    # Pattern: stack canary corruption.
    canary_events = [
        e for e in events if e.get("type") == "canary_check" and e.get("corrupted")
    ]
    if canary_events or "CANARY CORRUPTED" in log_text or "stack_chk_fail" in log_text:
        patterns.append(
            {
                "type": "stack_canary_corruption",
                "confidence": "high",
                "description": "Stack canary was corrupted — likely buffer overflow",
                "events": canary_events,
                "investigation": [
                    "Check the 'syscall' field to see which syscall was active",
                    "The overflow likely happened in the userspace function that called this syscall",
                    "Check buffer size calculations in the kernel's copy_to_user/copy_from_user paths",
                    "If 'when' is 'post_syscall', the kernel write path may be at fault",
                ],
            }
        )

    # Pattern: null pointer dereference.
    null_faults = [
        e
        for e in events
        if e.get("type") == "page_fault" and e.get("vaddr", -1) == 0
    ]
    if null_faults or "null pointer" in log_text.lower():
        patterns.append(
            {
                "type": "null_pointer_deref",
                "confidence": "high",
                "description": "Null pointer dereference detected",
                "events": null_faults,
                "investigation": [
                    "Check the 'ip' field to find which instruction caused the fault",
                    "Use resolve_address() on the IP to find the function",
                    "This usually means an Option::unwrap() on None or a zero-initialized pointer",
                ],
            }
        )

    # Pattern: unresolved page fault (SIGSEGV).
    segfaults = [
        e
        for e in events
        if e.get("type") == "page_fault"
        and not e.get("resolved")
        and e.get("reason") == "no_vma"
    ]
    if segfaults:
        patterns.append(
            {
                "type": "segmentation_fault",
                "confidence": "high",
                "description": f"{len(segfaults)} unresolved page fault(s) — process accessed unmapped memory",
                "events": segfaults[-5:],
                "investigation": [
                    "Check if the faulting address is near a VMA boundary (off-by-one in mmap/brk)",
                    "Check if the stack needs to grow (address near stack bottom)",
                    "Use resolve_address() on the 'ip' to find the faulting instruction",
                ],
            }
        )

    # Pattern: repeated ENOSYS (missing syscalls).
    enosys_events = [
        e
        for e in events
        if e.get("type") == "unimplemented_syscall"
    ]
    if enosys_events:
        unique_names = list({e.get("name", "?") for e in enosys_events})
        patterns.append(
            {
                "type": "missing_syscalls",
                "confidence": "medium",
                "description": f"{len(enosys_events)} call(s) to {len(unique_names)} unimplemented syscall(s)",
                "missing_syscalls": unique_names,
                "investigation": [
                    "These syscalls need to be implemented for the program to work correctly",
                    "Check if stubs (returning 0 or ENOSYS) are sufficient",
                    "Common stubs: epoll_create1, epoll_ctl, epoll_wait, prctl, mlock",
                ],
            }
        )

    # Pattern: panic.
    panic_events = [e for e in events if e.get("type") == "panic"]
    if panic_events or "panicked at" in log_text:
        for pe in panic_events:
            patterns.append(
                {
                    "type": "kernel_panic",
                    "confidence": "high",
                    "description": pe.get("message", "Unknown panic"),
                    "backtrace": pe.get("backtrace", []),
                    "investigation": [
                        "The backtrace shows the call chain that led to the panic",
                        "Use resolve_address() on backtrace addresses for source locations",
                        "Check if this is a service panic (catch_unwind should have caught it) or core panic",
                    ],
                }
            )

    # Pattern: usercopy fault (page fault during copy_to/from_user).
    ucopy_faults = [
        e for e in events if e.get("type") == "usercopy_fault"
    ]
    if ucopy_faults:
        # Group by context tag for analysis.
        by_ctx = {}
        for e in ucopy_faults:
            ctx = e.get("ctx", "unknown")
            by_ctx.setdefault(ctx, []).append(e)

        for ctx, ctx_events in by_ctx.items():
            labels = list({e.get("label", "?") for e in ctx_events})
            patterns.append(
                {
                    "type": "usercopy_fault",
                    "confidence": "high",
                    "description": (
                        f"{len(ctx_events)} usercopy fault(s) in '{ctx}' "
                        f"(phases: {', '.join(labels)})"
                    ),
                    "events": ctx_events[-5:],
                    "investigation": [
                        f"Context '{ctx}' identifies the kernel operation performing the copy",
                        "Check if the user address was valid and mapped",
                        "'trailing_bytes' label means copy was ≥8 bytes with a remainder — check the copy length",
                        "The fault_addr shows exactly which address couldn't be accessed",
                        "Use get_usercopy_trace() to see the full sequence of copies leading up to the fault",
                    ],
                }
            )

    # Pattern: SIGCHLD/signal issues.
    signal_terminates = [
        e
        for e in events
        if e.get("type") == "signal"
        and e.get("action") == "terminate"
        and e.get("signal_name") not in ("SIGCHLD", "SIGPIPE")
    ]
    if signal_terminates:
        patterns.append(
            {
                "type": "fatal_signals",
                "confidence": "medium",
                "description": f"{len(signal_terminates)} process(es) terminated by signal",
                "events": signal_terminates[-5:],
                "investigation": [
                    "Check if the signal was expected (e.g. SIGTERM for shutdown)",
                    "SIGSEGV + handler=None means the process didn't install a handler",
                    "Check for missing signal handler setup in the program",
                ],
            }
        )

    return patterns


# ── Main entry point ──


def main():
    global stream, gdb, symbols

    parser = argparse.ArgumentParser(
        description="Kevlar Kernel Debug MCP Server"
    )
    parser.add_argument(
        "--debug-log",
        help="Path to QEMU serial log or debug JSONL file (tailed in real-time)",
    )
    parser.add_argument(
        "--elf",
        default="kevlar.x64.elf",
        help="Path to kernel ELF (for GDB symbol loading)",
    )
    parser.add_argument(
        "--symbols",
        help="Path to .symbols file (for offline symbol resolution)",
    )
    parser.add_argument(
        "--gdb-port",
        type=int,
        default=7789,
        help="QEMU GDB stub port (default: 7789)",
    )
    parser.add_argument(
        "--no-gdb",
        action="store_true",
        help="Don't connect to GDB (event stream + offline analysis only)",
    )
    args = parser.parse_args()

    # Initialize event stream.
    stream = DebugEventStream()
    if args.debug_log:
        # Ingest existing data, then tail.
        stream.ingest_file(args.debug_log, follow=False)
        stream.ingest_file_background(args.debug_log)
        print(
            f"kevlar-debug: tailing {args.debug_log} ({len(stream.events)} events loaded)",
            file=sys.stderr,
        )

    # Initialize symbol resolver.
    sym_path = args.symbols
    if not sym_path:
        # Try to auto-detect.
        for candidate in ["kevlar.x64.symbols", "kevlar.arm64.symbols"]:
            if Path(candidate).exists():
                sym_path = candidate
                break
    if sym_path and Path(sym_path).exists():
        symbols = SymbolResolver(sym_path)
        print(
            f"kevlar-debug: loaded {len(symbols.symbols)} symbols from {sym_path}",
            file=sys.stderr,
        )

    # Initialize GDB bridge.
    if not args.no_gdb:
        gdb = GDBBridge(gdb_port=args.gdb_port, elf_path=args.elf)
        if gdb.connect():
            print(
                f"kevlar-debug: connected to GDB on port {args.gdb_port}",
                file=sys.stderr,
            )
        else:
            print(
                f"kevlar-debug: GDB not available on port {args.gdb_port} (live inspection disabled)",
                file=sys.stderr,
            )

    # Run the MCP server.
    print("kevlar-debug: MCP server starting...", file=sys.stderr)
    mcp.run()


if __name__ == "__main__":
    main()
