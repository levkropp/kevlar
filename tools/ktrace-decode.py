#!/usr/bin/env python3
"""ktrace-decode.py — Decode Kevlar ktrace binary dumps.

Reads the binary output from QEMU's ISA debugcon device and produces:
- Text timeline (default)
- Summary statistics (--summary)
- Perfetto JSON for visualization (--perfetto FILE)

Usage:
    python3 tools/ktrace-decode.py ktrace.bin
    python3 tools/ktrace-decode.py ktrace.bin --summary
    python3 tools/ktrace-decode.py ktrace.bin --timeline --pid 4
    python3 tools/ktrace-decode.py ktrace.bin --perfetto trace.json
"""

import argparse
import json
import struct
import sys
from collections import defaultdict

# ── Binary format ────────────────────────────────────────────────────────

HEADER_SIZE = 64
HEADER_MAGIC = b"KTRX"
ENTRY_SIZE = 32

# Header: magic(4) + version(4) + tsc_freq(8) + num_cpus(4) + ring_size(4) + entry_size(4) + flags(4) + reserved(32)
HEADER_FMT = "<4sIQIIII32s"

# Entry: tsc(8) + header(4) + data[5](20)
ENTRY_FMT = "<QI5I"

# ── Event type names ─────────────────────────────────────────────────────

EVENT_NAMES = {
    0: "SYSCALL_ENTER",
    1: "SYSCALL_EXIT",
    5: "CTX_SWITCH",
    10: "PAGE_FAULT",
    70: "WAITQ_SLEEP",
    71: "WAITQ_WAKE",
    193: "NET_CONNECT",
    197: "NET_SEND",
    198: "NET_RECV",
    199: "NET_POLL",
    201: "NET_RX_PACKET",
    202: "NET_TX_PACKET",
    203: "NET_TCP_STATE",
    204: "NET_DNS_QUERY",
}

# x86_64 syscall numbers → names (most common).
SYSCALL_NAMES = {
    0: "read", 1: "write", 2: "open", 3: "close", 4: "stat", 5: "fstat",
    6: "lstat", 7: "poll", 8: "lseek", 9: "mmap", 10: "mprotect",
    11: "munmap", 12: "brk", 13: "rt_sigaction", 14: "rt_sigprocmask",
    15: "rt_sigreturn", 16: "ioctl", 17: "pread64", 20: "writev",
    21: "access", 22: "pipe", 23: "select", 24: "sched_yield", 25: "mremap",
    28: "madvise", 32: "dup", 33: "dup2", 35: "nanosleep",
    39: "getpid", 41: "socket", 42: "connect", 43: "accept", 44: "sendto",
    45: "recvfrom", 46: "sendmsg", 47: "recvmsg", 48: "shutdown", 49: "bind",
    50: "listen", 51: "getsockname", 52: "getpeername", 53: "socketpair",
    54: "setsockopt", 55: "getsockopt", 57: "fork", 58: "vfork", 59: "execve",
    60: "exit", 61: "wait4", 62: "kill", 63: "uname", 72: "fcntl",
    79: "getcwd", 80: "chdir", 83: "mkdir", 84: "rmdir", 87: "unlink",
    89: "readlink", 102: "getuid", 104: "getgid", 110: "getppid",
    158: "arch_prctl", 186: "gettid", 202: "futex", 218: "set_tid_address",
    228: "clock_gettime", 231: "exit_group", 257: "openat", 262: "newfstatat",
    270: "pselect6", 281: "epoll_pwait", 288: "accept4", 290: "eventfd2",
    291: "epoll_create1", 292: "dup3", 293: "pipe2", 302: "preadv",
    318: "getrandom",
}


def ip_from_u32(val):
    """Convert a u32 (big-endian IP in host order) to dotted-quad string."""
    return f"{(val >> 24) & 0xFF}.{(val >> 16) & 0xFF}.{(val >> 8) & 0xFF}.{val & 0xFF}"


def signed_i64_from_u32_pair(lo, hi):
    """Reconstruct a signed i64 from two u32 halves."""
    val = lo | (hi << 32)
    if val >= (1 << 63):
        val -= (1 << 64)
    return val


# ── Parsing ──────────────────────────────────────────────────────────────

def parse_header(data):
    """Parse the 64-byte dump header. Returns dict or None on error."""
    if len(data) < HEADER_SIZE:
        return None
    fields = struct.unpack(HEADER_FMT, data[:HEADER_SIZE])
    magic, version, tsc_freq, num_cpus, ring_size, entry_size, flags, _ = fields
    if magic != HEADER_MAGIC:
        return None
    return {
        "version": version,
        "tsc_freq_hz": tsc_freq,
        "num_cpus": num_cpus,
        "ring_size": ring_size,
        "entry_size": entry_size,
        "flags": flags,
    }


def parse_entry(data):
    """Parse a 32-byte trace record. Returns dict."""
    tsc, header, d0, d1, d2, d3, d4 = struct.unpack(ENTRY_FMT, data[:ENTRY_SIZE])
    event_type = header & 0x3FF
    cpu = (header >> 10) & 0x7
    pid_idx = (header >> 13) & 0x7FF
    flags = (header >> 24) & 0xFF
    return {
        "tsc": tsc,
        "event_type": event_type,
        "cpu": cpu,
        "pid": pid_idx,
        "flags": flags,
        "data": [d0, d1, d2, d3, d4],
    }


def parse_dump(filepath):
    """Parse a full ktrace binary dump. Returns (header, list_of_entries).

    The dump file may contain multiple concatenated dumps (initial + final).
    We use the LAST valid dump since it has the most data.
    """
    with open(filepath, "rb") as f:
        raw = f.read()

    # Find all KTRX headers in the file and use the last one.
    last_header = None
    last_header_offset = 0
    search_offset = 0
    while search_offset + HEADER_SIZE <= len(raw):
        idx = raw.find(HEADER_MAGIC, search_offset)
        if idx < 0:
            break
        h = parse_header(raw[idx:])
        if h is not None:
            last_header = h
            last_header_offset = idx
        search_offset = idx + 1

    if last_header is None:
        print(f"Error: {filepath} does not have a valid KTRX header", file=sys.stderr)
        sys.exit(1)

    header = last_header
    entries = []
    offset = last_header_offset + HEADER_SIZE
    num_cpus = header["num_cpus"]
    ring_size = header["ring_size"]

    for cpu in range(num_cpus):
        for i in range(ring_size):
            if offset + ENTRY_SIZE > len(raw):
                break
            entry = parse_entry(raw[offset:offset + ENTRY_SIZE])
            if entry["tsc"] != 0:  # skip uninitialized slots
                entries.append(entry)
            offset += ENTRY_SIZE

    # Sort by TSC for a global timeline.
    entries.sort(key=lambda e: e["tsc"])
    return header, entries


# ── Formatters ───────────────────────────────────────────────────────────

def format_event(entry, tsc_freq, base_tsc):
    """Format a single event as a human-readable line."""
    event_type = entry["event_type"]
    name = EVENT_NAMES.get(event_type, f"EVENT_{event_type}")
    cpu = entry["cpu"]
    pid = entry["pid"]
    d = entry["data"]

    # Timestamp relative to first event, in seconds.
    dt = (entry["tsc"] - base_tsc) / tsc_freq if tsc_freq > 0 else 0.0

    detail = ""
    if event_type == 0:  # SYSCALL_ENTER
        nr = d[0]
        a1 = d[1] | (d[2] << 32)
        a2 = d[3] | (d[4] << 32)
        sc_name = SYSCALL_NAMES.get(nr, f"syscall_{nr}")
        detail = f"nr={nr} ({sc_name}) a1=0x{a1:x} a2=0x{a2:x}"
    elif event_type == 1:  # SYSCALL_EXIT
        nr = d[0]
        result = signed_i64_from_u32_pair(d[1], d[2])
        sc_name = SYSCALL_NAMES.get(nr, f"syscall_{nr}")
        detail = f"nr={nr} ({sc_name}) result={result}"
    elif event_type == 5:  # CTX_SWITCH
        detail = f"from_pid={d[0]} to_pid={d[1]}"
    elif event_type == 70:  # WAITQ_SLEEP
        detail = f"waitq=0x{d[0]:x}"
    elif event_type == 71:  # WAITQ_WAKE
        detail = f"waitq=0x{d[0]:x} woken={d[1]}"
    elif event_type == 193:  # NET_CONNECT
        detail = f"fd={d[0]} ip={ip_from_u32(d[1])} port={d[2]} result={d[3]}"
    elif event_type == 197:  # NET_SEND
        detail = f"fd={d[0]} len={d[1]} result={d[2]}"
    elif event_type == 198:  # NET_RECV
        detail = f"fd={d[0]} len={d[1]} result={d[2]}"
    elif event_type == 199:  # NET_POLL
        detail = f"fd={d[0]} revents=0x{d[2]:x}"
    elif event_type == 201:  # NET_RX_PACKET
        detail = f"frame_len={d[0]}"
    elif event_type == 202:  # NET_TX_PACKET
        detail = f"frame_len={d[0]}"
    elif event_type == 10:  # PAGE_FAULT
        addr = d[0] | (d[1] << 32)
        rip = d[2] | (d[3] << 32)
        reason = d[4]
        reason_parts = []
        if reason & 1: reason_parts.append("PRESENT")
        if reason & 2: reason_parts.append("WRITE")
        if reason & 4: reason_parts.append("USER")
        if reason & 16: reason_parts.append("INST_FETCH")
        reason_str = "|".join(reason_parts) if reason_parts else f"0x{reason:x}"
        detail = f"addr=0x{addr:x} rip=0x{rip:x} reason={reason_str}"
    elif event_type == 203:  # NET_TCP_STATE
        detail = f"handle={d[0]} old={d[1]} new={d[2]}"
    else:
        detail = " ".join(f"d{i}=0x{v:x}" for i, v in enumerate(d))

    return f"[{dt:12.6f}] CPU{cpu} PID={pid:<4} {name:<16} {detail}"


def cmd_timeline(header, entries, args):
    """Print a text timeline of events."""
    base_tsc = entries[0]["tsc"] if entries else 0
    tsc_freq = header["tsc_freq_hz"]

    for entry in entries:
        if args.pid is not None and entry["pid"] != args.pid:
            continue
        if args.cpu is not None and entry["cpu"] != args.cpu:
            continue
        print(format_event(entry, tsc_freq, base_tsc))


def cmd_summary(header, entries, args):
    """Print summary statistics."""
    print(f"ktrace dump: version={header['version']} cpus={header['num_cpus']} "
          f"ring_size={header['ring_size']} tsc_freq={header['tsc_freq_hz']}")
    print(f"Total events: {len(entries)}")

    by_type = defaultdict(int)
    by_pid = defaultdict(int)
    by_cpu = defaultdict(int)

    for e in entries:
        by_type[e["event_type"]] += 1
        by_pid[e["pid"]] += 1
        by_cpu[e["cpu"]] += 1

    print("\nEvents by type:")
    for ty, count in sorted(by_type.items(), key=lambda x: -x[1]):
        name = EVENT_NAMES.get(ty, f"EVENT_{ty}")
        print(f"  {name:<20} {count:>8}")

    print("\nEvents by PID:")
    for pid, count in sorted(by_pid.items(), key=lambda x: -x[1])[:20]:
        print(f"  PID {pid:<6} {count:>8}")

    print("\nEvents by CPU:")
    for cpu, count in sorted(by_cpu.items()):
        print(f"  CPU{cpu}  {count:>8}")

    if entries:
        tsc_freq = header["tsc_freq_hz"]
        span = (entries[-1]["tsc"] - entries[0]["tsc"]) / tsc_freq if tsc_freq else 0
        print(f"\nTime span: {span:.6f}s ({len(entries)/span:.0f} events/s)" if span > 0 else "")


def cmd_perfetto(header, entries, args):
    """Export to Perfetto JSON trace format."""
    tsc_freq = header["tsc_freq_hz"]
    base_tsc = entries[0]["tsc"] if entries else 0

    trace_events = []
    # Track open syscalls for duration events.
    open_syscalls = {}  # (cpu, pid) -> (tsc, nr)

    for entry in entries:
        cpu = entry["cpu"]
        pid = entry["pid"]
        et = entry["event_type"]
        d = entry["data"]
        ts_us = (entry["tsc"] - base_tsc) * 1_000_000 / tsc_freq if tsc_freq else 0

        if et == 0:  # SYSCALL_ENTER
            nr = d[0]
            sc_name = SYSCALL_NAMES.get(nr, f"syscall_{nr}")
            open_syscalls[(cpu, pid)] = (ts_us, nr, sc_name)
            trace_events.append({
                "ph": "B", "ts": ts_us, "pid": pid, "tid": cpu,
                "name": sc_name, "cat": "syscall",
            })
        elif et == 1:  # SYSCALL_EXIT
            result = signed_i64_from_u32_pair(d[1], d[2])
            nr = d[0]
            sc_name = SYSCALL_NAMES.get(nr, f"syscall_{nr}")
            trace_events.append({
                "ph": "E", "ts": ts_us, "pid": pid, "tid": cpu,
                "name": sc_name, "cat": "syscall",
                "args": {"result": result},
            })
            open_syscalls.pop((cpu, pid), None)
        elif et == 5:  # CTX_SWITCH
            trace_events.append({
                "ph": "i", "ts": ts_us, "pid": pid, "tid": cpu,
                "name": "CTX_SWITCH", "cat": "sched", "s": "t",
                "args": {"from": d[0], "to": d[1]},
            })
        elif et == 70:  # WAITQ_SLEEP
            trace_events.append({
                "ph": "B", "ts": ts_us, "pid": pid, "tid": cpu,
                "name": "SLEEP", "cat": "sched",
                "args": {"waitq": f"0x{d[0]:x}"},
            })
        elif et == 71:  # WAITQ_WAKE
            trace_events.append({
                "ph": "E", "ts": ts_us, "pid": pid, "tid": cpu,
                "name": "SLEEP", "cat": "sched",
                "args": {"woken": d[1]},
            })
        elif et == 10:  # PAGE_FAULT
            addr = d[0] | (d[1] << 32)
            rip = d[2] | (d[3] << 32)
            trace_events.append({
                "ph": "i", "ts": ts_us, "pid": pid, "tid": cpu,
                "name": "PAGE_FAULT", "cat": "mm", "s": "t",
                "args": {"addr": f"0x{addr:x}", "rip": f"0x{rip:x}", "reason": d[4]},
            })
        elif 193 <= et <= 210:  # Network events
            name = EVENT_NAMES.get(et, f"NET_{et}")
            trace_events.append({
                "ph": "i", "ts": ts_us, "pid": pid, "tid": cpu,
                "name": name, "cat": "net", "s": "t",
                "args": {f"d{i}": v for i, v in enumerate(d)},
            })

    # Add metadata for process/thread naming.
    trace_events.append({
        "ph": "M", "pid": 0, "name": "process_name",
        "args": {"name": "Kevlar Kernel"},
    })
    for cpu in range(header["num_cpus"]):
        trace_events.append({
            "ph": "M", "pid": 0, "tid": cpu, "name": "thread_name",
            "args": {"name": f"CPU{cpu}"},
        })

    output = {"traceEvents": trace_events}
    outpath = args.perfetto
    with open(outpath, "w") as f:
        json.dump(output, f)
    print(f"Wrote {len(trace_events)} events to {outpath}", file=sys.stderr)
    print(f"Open in https://ui.perfetto.dev", file=sys.stderr)


# ── Main ─────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Decode Kevlar ktrace binary dumps")
    parser.add_argument("dump_file", help="Path to ktrace.bin")
    parser.add_argument("--summary", action="store_true", help="Print event count summary")
    parser.add_argument("--timeline", action="store_true", help="Print text timeline (default)")
    parser.add_argument("--perfetto", metavar="FILE", help="Export Perfetto JSON trace")
    parser.add_argument("--pid", type=int, help="Filter by PID")
    parser.add_argument("--cpu", type=int, help="Filter by CPU")
    args = parser.parse_args()

    header, entries = parse_dump(args.dump_file)

    if not entries:
        print("No trace events found.", file=sys.stderr)
        sys.exit(0)

    if args.summary:
        cmd_summary(header, entries, args)
    elif args.perfetto:
        cmd_perfetto(header, entries, args)
    else:
        cmd_timeline(header, entries, args)


if __name__ == "__main__":
    main()
