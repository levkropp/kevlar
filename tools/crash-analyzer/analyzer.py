#!/usr/bin/env python3
"""
Kevlar Kernel Crash Analyzer

Offline analysis tool for crash dumps, serial logs, and debug event streams.
Produces structured JSON output optimized for LLM consumption.

Usage:
    # Analyze a crash dump:
    python3 tools/crash-analyzer/analyzer.py dump kevlar.dump --symbols kevlar.x64.symbols

    # Analyze a serial log:
    python3 tools/crash-analyzer/analyzer.py log /tmp/kevlar-bench-balanced.log

    # Analyze a debug event stream (JSONL):
    python3 tools/crash-analyzer/analyzer.py events debug.jsonl

    # All output is JSON — pipe to jq for pretty-printing:
    python3 tools/crash-analyzer/analyzer.py log serial.log | jq .
"""

import argparse
import json
import struct
import sys
from pathlib import Path
from typing import Optional


class SymbolResolver:
    """Offline symbol resolution from .symbols file."""

    def __init__(self, path: Optional[str] = None):
        self.symbols: list[tuple[int, str]] = []
        if path:
            self._load(path)

    def _load(self, path: str) -> None:
        for line in Path(path).read_text().splitlines():
            parts = line.strip().split(None, 1)
            if len(parts) == 2:
                try:
                    self.symbols.append((int(parts[0], 16), parts[1].strip()))
                except ValueError:
                    continue
        self.symbols.sort(key=lambda x: x[0])

    def resolve(self, addr: int) -> dict:
        if not self.symbols:
            return {"addr": addr, "symbol": "?", "offset": 0}
        lo, hi = 0, len(self.symbols) - 1
        while lo <= hi:
            mid = (lo + hi) // 2
            if self.symbols[mid][0] <= addr:
                lo = mid + 1
            else:
                hi = mid - 1
        if hi >= 0:
            base, name = self.symbols[hi]
            return {"addr": addr, "symbol": name, "offset": addr - base}
        return {"addr": addr, "symbol": "?", "offset": 0}


def parse_dump(dump_path: str) -> dict:
    """Parse a kevlar.dump (boot2dump format)."""
    data = Path(dump_path).read_bytes()
    if len(data) < 8:
        return {"error": "Dump file too small"}

    magic = struct.unpack_from("<I", data, 0)[0]
    if magic != 0xDEADBEEE:
        return {"error": f"Bad magic: {magic:#x}"}

    log_len = struct.unpack_from("<I", data, 4)[0]
    log_bytes = data[8 : 8 + min(log_len, 4096)]
    log_text = log_bytes.decode("utf-8", errors="replace")

    return {
        "magic": f"{magic:#x}",
        "log_length": log_len,
        "log_text": log_text,
    }


def parse_events_from_text(text: str) -> tuple[list[dict], list[str]]:
    """Split text into structured events and plain log lines."""
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
    return events, log_lines


def detect_patterns(
    log_text: str, events: list[dict], resolver: SymbolResolver
) -> list[dict]:
    """Detect known crash/bug patterns."""
    patterns = []

    # ── Stack canary corruption ──
    canary_events = [
        e for e in events if e.get("type") == "canary_check" and e.get("corrupted")
    ]
    if canary_events or "CANARY CORRUPTED" in log_text or "__stack_chk_fail" in log_text:
        info = {
            "type": "stack_canary_corruption",
            "severity": "critical",
            "count": max(len(canary_events), 1),
            "description": "Stack canary corruption detected — buffer overflow in userspace",
        }
        if canary_events:
            last = canary_events[-1]
            info["last_corruption"] = {
                "pid": last.get("pid"),
                "syscall": last.get("syscall"),
                "when": last.get("when"),
                "expected": last.get("expected"),
                "found": last.get("found"),
                "fsbase": last.get("fsbase"),
            }
            info["diagnosis"] = (
                f"Canary changed from {last.get('expected', '?'):#x} to "
                f"{last.get('found', '?'):#x} during/after {last.get('syscall', '?')}() syscall. "
                f"The kernel's copy_to_user in this syscall likely wrote past "
                f"the user buffer, overwriting the TLS canary at fsbase+0x28."
            )
        patterns.append(info)

    # ── Null pointer dereference ──
    null_faults = [
        e for e in events
        if e.get("type") == "page_fault" and e.get("vaddr", -1) == 0
    ]
    if null_faults or "null pointer access" in log_text.lower():
        info = {
            "type": "null_pointer_deref",
            "severity": "critical",
            "count": max(len(null_faults), 1),
        }
        if null_faults:
            last = null_faults[-1]
            sym = resolver.resolve(last.get("ip", 0))
            info["last_fault"] = {
                "pid": last.get("pid"),
                "ip": last.get("ip"),
                "ip_symbol": sym,
            }
            info["diagnosis"] = (
                f"Process {last.get('pid')} dereferenced a null pointer at "
                f"IP {last.get('ip', 0):#x} ({sym['symbol']}+{sym['offset']:#x})"
            )
        patterns.append(info)

    # ── Segmentation fault (no VMA) ──
    segfaults = [
        e for e in events
        if e.get("type") == "page_fault"
        and not e.get("resolved")
        and e.get("reason") == "no_vma"
    ]
    if segfaults:
        last = segfaults[-1]
        sym = resolver.resolve(last.get("ip", 0))
        patterns.append({
            "type": "segfault_no_vma",
            "severity": "critical",
            "count": len(segfaults),
            "last_fault": {
                "pid": last.get("pid"),
                "vaddr": last.get("vaddr"),
                "ip": last.get("ip"),
                "ip_symbol": sym,
            },
            "diagnosis": (
                f"Process {last.get('pid')} accessed {last.get('vaddr', 0):#x} "
                f"which has no VMA mapping. IP: {sym['symbol']}+{sym['offset']:#x}"
            ),
        })

    # ── Kernel panic ──
    panics = [e for e in events if e.get("type") == "panic"]
    for pe in panics:
        bt = pe.get("backtrace", [])
        resolved_bt = []
        for frame in bt:
            addr = frame.get("addr", 0)
            sym = resolver.resolve(addr)
            resolved_bt.append({
                "addr": f"{addr:#x}",
                "symbol": sym["symbol"],
                "offset": f"+{sym['offset']:#x}",
            })
        patterns.append({
            "type": "kernel_panic",
            "severity": "critical",
            "message": pe.get("message", "?"),
            "backtrace": resolved_bt,
        })

    # ── Unimplemented syscalls ──
    unimpl = [e for e in events if e.get("type") == "unimplemented_syscall"]
    if unimpl:
        names = sorted({e.get("name", "?") for e in unimpl})
        patterns.append({
            "type": "missing_syscalls",
            "severity": "warning",
            "count": len(unimpl),
            "syscalls": names,
            "diagnosis": (
                f"Programs called {len(names)} unimplemented syscall(s): "
                f"{', '.join(names)}. These return ENOSYS."
            ),
        })

    # ── Usercopy faults ──
    ucopy_faults = [e for e in events if e.get("type") == "usercopy_fault"]
    if ucopy_faults:
        by_ctx: dict[str, list[dict]] = {}
        for e in ucopy_faults:
            ctx = e.get("ctx", "unknown")
            by_ctx.setdefault(ctx, []).append(e)

        for ctx, ctx_events in by_ctx.items():
            last = ctx_events[-1]
            labels = sorted({e.get("label", "?") for e in ctx_events})
            sym = resolver.resolve(last.get("ip", 0))
            patterns.append({
                "type": "usercopy_fault",
                "severity": "critical",
                "count": len(ctx_events),
                "context": ctx,
                "usercopy_phases": labels,
                "last_fault": {
                    "pid": last.get("pid"),
                    "fault_addr": last.get("fault_addr"),
                    "ip": last.get("ip"),
                    "ip_symbol": sym,
                    "label": last.get("label"),
                    "dst": last.get("dst"),
                    "src": last.get("src"),
                    "remaining": last.get("remaining"),
                },
                "diagnosis": (
                    f"Page fault during usercopy in '{ctx}' "
                    f"(phase: {', '.join(labels)}). "
                    f"Fault at {last.get('fault_addr', 0):#x}, "
                    f"IP: {sym['symbol']}+{sym['offset']:#x}. "
                    f"This means the kernel tried to access an unmapped user address."
                ),
            })

    # ── Signal terminations ──
    sig_terms = [
        e for e in events
        if e.get("type") == "signal"
        and e.get("action") == "terminate"
        and e.get("signal_name") not in ("SIGCHLD", "SIGPIPE")
    ]
    if sig_terms:
        patterns.append({
            "type": "fatal_signals",
            "severity": "high",
            "count": len(sig_terms),
            "events": sig_terms[-5:],
            "diagnosis": (
                f"{len(sig_terms)} process(es) killed by fatal signals: "
                + ", ".join(
                    f"pid={e.get('pid')} by {e.get('signal_name')}"
                    for e in sig_terms[-3:]
                )
            ),
        })

    return patterns


def build_summary(events: list[dict]) -> dict:
    """Build a statistical summary of debug events."""
    type_counts: dict[str, int] = {}
    for e in events:
        t = e.get("type", "unknown")
        type_counts[t] = type_counts.get(t, 0) + 1

    syscall_errors: dict[str, dict[str, int]] = {}
    for e in events:
        if e.get("type") == "syscall_exit" and "errno" in e:
            name = e.get("name", "?")
            errno = e["errno"]
            syscall_errors.setdefault(name, {})
            syscall_errors[name][errno] = syscall_errors[name].get(errno, 0) + 1

    unique_pids = {e.get("pid") for e in events if "pid" in e}

    return {
        "total_events": len(events),
        "event_types": type_counts,
        "unique_pids": sorted(p for p in unique_pids if p is not None),
        "syscall_error_summary": syscall_errors,
    }


def cmd_dump(args):
    """Analyze a crash dump file."""
    dump = parse_dump(args.dump_path)
    if "error" in dump:
        print(json.dumps(dump, indent=2))
        return 1

    events, log_lines = parse_events_from_text(dump["log_text"])
    resolver = SymbolResolver(args.symbols)
    patterns = detect_patterns(dump["log_text"], events, resolver)

    result = {
        "source": args.dump_path,
        "format": "boot2dump",
        "log_length": dump["log_length"],
        "last_log_lines": log_lines[-30:],
        "structured_events": events,
        "detected_patterns": patterns,
        "summary": build_summary(events),
    }

    print(json.dumps(result, indent=2))
    return 0


def cmd_log(args):
    """Analyze a serial log file."""
    text = Path(args.log_path).read_text(errors="replace")
    events, log_lines = parse_events_from_text(text)
    resolver = SymbolResolver(args.symbols)
    patterns = detect_patterns(text, events, resolver)

    result = {
        "source": args.log_path,
        "format": "serial_log",
        "total_lines": len(text.splitlines()),
        "last_log_lines": log_lines[-30:],
        "detected_patterns": patterns,
        "summary": build_summary(events),
    }

    print(json.dumps(result, indent=2))
    return 0


def cmd_events(args):
    """Analyze a JSONL debug event file."""
    text = Path(args.events_path).read_text(errors="replace")
    events, log_lines = parse_events_from_text(text)
    resolver = SymbolResolver(args.symbols)
    patterns = detect_patterns(text, events, resolver)

    result = {
        "source": args.events_path,
        "format": "jsonl_events",
        "detected_patterns": patterns,
        "summary": build_summary(events),
    }

    print(json.dumps(result, indent=2))
    return 0


def main():
    parser = argparse.ArgumentParser(
        description="Kevlar Kernel Crash Analyzer",
        epilog="All output is JSON. Pipe to 'jq .' for pretty-printing.",
    )
    parser.add_argument(
        "--symbols", help="Path to .symbols file for address resolution"
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_dump = sub.add_parser("dump", help="Analyze a kevlar.dump crash dump")
    p_dump.add_argument("dump_path")

    p_log = sub.add_parser("log", help="Analyze a QEMU serial log")
    p_log.add_argument("log_path")

    p_events = sub.add_parser("events", help="Analyze a debug event JSONL file")
    p_events.add_argument("events_path")

    args = parser.parse_args()

    handlers = {"dump": cmd_dump, "log": cmd_log, "events": cmd_events}
    sys.exit(handlers[args.command](args))


if __name__ == "__main__":
    main()
