#!/usr/bin/env python3
"""Parse Kevlar crash reports from serial output and display human-readable analysis.

Usage:
  # From a log file:
  python3 tools/crash-report.py debug.log

  # From stdin (pipe QEMU output):
  python3 tools/run-qemu.py ... --append-cmdline "debug=fault,process" ... 2>&1 | python3 tools/crash-report.py

  # With symbol resolution:
  python3 tools/crash-report.py debug.log --binary /tmp/apk.static

  # JSON output:
  python3 tools/crash-report.py debug.log --json
"""
import json
import re
import subprocess
import sys
import os

DBG_RE = re.compile(r'^DBG\s+(\{.+\})\s*$')
# Match 0xHEX values that are NOT inside quotes (JSON values, not strings).
HEX_RE = re.compile(r'(?<=[:,\[])0x([0-9a-fA-F]+)')

def fix_hex_json(s):
    """Convert 0xHEX literals in pseudo-JSON to decimal for json.loads()."""
    return HEX_RE.sub(lambda m: str(int(m.group(1), 16)), s)

def parse_events(stream):
    """Parse DBG JSONL events from a stream of lines."""
    events = []
    for line in stream:
        line = line.rstrip('\n\r')
        # Strip ANSI escape codes
        line = re.sub(r'\x1b\[[0-9;]*m', '', line)
        m = DBG_RE.match(line)
        if m:
            try:
                fixed = fix_hex_json(m.group(1))
                events.append(json.loads(fixed))
            except json.JSONDecodeError:
                pass
    return events

def find_crashes(events):
    """Find crash_report events."""
    return [e for e in events if e.get('type') == 'crash_report']

def signal_name(sig):
    names = {
        1: 'SIGHUP', 2: 'SIGINT', 3: 'SIGQUIT', 4: 'SIGILL',
        5: 'SIGTRAP', 6: 'SIGABRT', 7: 'SIGBUS', 8: 'SIGFPE',
        9: 'SIGKILL', 10: 'SIGUSR1', 11: 'SIGSEGV', 12: 'SIGUSR2',
        13: 'SIGPIPE', 14: 'SIGALRM', 15: 'SIGTERM',
    }
    return names.get(sig, f'signal {sig}')

def resolve_addr(binary, addr):
    """Try to resolve an address to a symbol using nm or objdump."""
    if not binary or not os.path.exists(binary):
        return None
    try:
        result = subprocess.run(
            ['addr2line', '-f', '-e', binary, hex(addr)],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0:
            lines = result.stdout.strip().split('\n')
            if len(lines) >= 2 and lines[0] != '??' and lines[1] != '??:0':
                return f"{lines[0]} at {lines[1]}"
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Fallback: use nm to find nearest symbol
    try:
        result = subprocess.run(
            ['nm', '-n', binary], capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0:
            prev_sym = None
            for line in result.stdout.split('\n'):
                parts = line.split()
                if len(parts) >= 3:
                    sym_addr = int(parts[0], 16)
                    sym_name = parts[2]
                    if sym_addr > addr and prev_sym:
                        offset = addr - prev_sym[0]
                        return f"{prev_sym[1]}+{offset:#x}"
                    prev_sym = (sym_addr, sym_name)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return None

def disassemble_around(binary, addr, context=8):
    """Disassemble a few instructions around the crash address."""
    if not binary or not os.path.exists(binary):
        return None
    try:
        start = max(0, addr - context * 4)
        result = subprocess.run(
            ['objdump', '-d',
             f'--start-address={start:#x}',
             f'--stop-address={addr + context * 8:#x}',
             binary],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0:
            lines = []
            in_section = False
            for line in result.stdout.split('\n'):
                if line.strip().startswith(f'{addr:x}:') or line.strip().startswith(f'{addr:08x}:'):
                    in_section = True
                if in_section or (line.strip() and ':' in line and '\t' in line):
                    # Only include actual disassembly lines
                    stripped = line.strip()
                    if stripped and ':' in stripped[:20]:
                        marker = '>>>' if stripped.startswith(f'{addr:x}:') else '   '
                        lines.append(f"  {marker} {stripped}")
                        if len(lines) > context * 2:
                            break
            return '\n'.join(lines) if lines else None
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass
    return None

SYSCALL_NAMES = {
    0: 'read', 1: 'write', 2: 'open', 3: 'close', 5: 'fstat',
    8: 'lseek', 9: 'mmap', 10: 'mprotect', 11: 'munmap', 12: 'brk',
    13: 'rt_sigaction', 14: 'rt_sigprocmask', 16: 'ioctl',
    21: 'access', 56: 'clone', 57: 'fork', 59: 'execve',
    60: 'exit', 63: 'uname', 72: 'fcntl', 79: 'getcwd',
    87: 'unlink', 88: 'symlink', 89: 'readlink',
    96: 'gettimeofday', 102: 'getuid', 104: 'getgid',
    137: 'statfs', 138: 'fstatfs', 158: 'arch_prctl',
    186: 'gettid', 202: 'futex', 217: 'getdents64',
    218: 'set_tid_address', 231: 'exit_group',
    257: 'openat', 262: 'newfstatat',
}

def format_crash(crash, binary=None):
    """Format a crash report as human-readable text."""
    lines = []
    pid = crash['pid']
    sig = crash['signal']
    sname = crash.get('signal_name', signal_name(sig))
    cmdline = crash.get('cmdline', '(unknown)')
    fault = crash.get('fault_addr', 0)
    ip = crash.get('ip', 0)

    lines.append(f"{'='*72}")
    lines.append(f"  CRASH REPORT: PID {pid} ({cmdline}) killed by {sname}")
    lines.append(f"{'='*72}")
    lines.append(f"")
    lines.append(f"  Fault address: {fault:#x}")
    lines.append(f"  Instruction:   {ip:#x}")
    lines.append(f"  FS base:       {crash.get('fsbase', 0):#x}")

    # Symbol resolution
    if binary and ip:
        sym = resolve_addr(binary, ip)
        if sym:
            lines.append(f"  Symbol:        {sym}")

    # Registers
    regs = crash.get('regs', {})
    if any(v != 0 for v in regs.values()):
        lines.append(f"")
        lines.append(f"  Registers:")
        for row in [
            ['rax', 'rbx', 'rcx', 'rdx'],
            ['rsi', 'rdi', 'rbp', 'rsp'],
            ['r8', 'r9', 'r10', 'r11'],
            ['r12', 'r13', 'r14', 'r15'],
        ]:
            vals = '  '.join(f"{r}={regs.get(r, 0):#018x}" for r in row)
            lines.append(f"    {vals}")
        lines.append(f"    rflags={regs.get('rflags', 0):#018x}")

    # Disassembly
    if binary and ip:
        disasm = disassemble_around(binary, ip)
        if disasm:
            lines.append(f"")
            lines.append(f"  Disassembly around {ip:#x}:")
            lines.append(disasm)

    # Last syscalls
    syscalls = crash.get('syscalls', [])
    if syscalls:
        lines.append(f"")
        lines.append(f"  Last {len(syscalls)} syscalls (oldest first):")
        for i, sc in enumerate(syscalls):
            if isinstance(sc, dict):
                nr = sc.get('nr', 0)
                name = sc.get('name', SYSCALL_NAMES.get(nr, f'syscall_{nr}'))
                result = sc.get('result', 0)
                a0 = sc.get('a0', 0)
                a1 = sc.get('a1', 0)
            else:
                nr, result, a0, a1 = sc
                name = SYSCALL_NAMES.get(nr, f'syscall_{nr}')

            if result < 0:
                result_str = f"{result} (errno {-result})"
            else:
                result_str = f"{result:#x}" if result > 255 else str(result)
            lines.append(f"    [{i:2d}] {name}({a0:#x}, {a1:#x}) -> {result_str}")

    # VMAs
    vmas = crash.get('vmas', [])
    if vmas:
        lines.append(f"")
        lines.append(f"  Memory map ({len(vmas)} VMAs):")
        for v in vmas:
            if isinstance(v, dict):
                start = v.get('start', 0)
                end = v.get('end', 0)
                vtype = v.get('type', '?')
            else:
                start, end, vtype = v
            size = end - start
            if size >= 1024 * 1024:
                size_str = f"{size // 1024 // 1024}M"
            elif size >= 1024:
                size_str = f"{size // 1024}K"
            else:
                size_str = f"{size}"
            lines.append(f"    {start:#014x}-{end:#014x} ({size_str:>6s}) {vtype}")

        # Highlight the VMA containing the fault address
        if fault:
            for v in vmas:
                s = v.get('start', v[0] if isinstance(v, (list, tuple)) else 0)
                e = v.get('end', v[1] if isinstance(v, (list, tuple)) else 0)
                if s <= fault < e:
                    lines.append(f"  -> Fault address {fault:#x} is in VMA {s:#x}-{e:#x}")
                    break
            else:
                lines.append(f"  -> Fault address {fault:#x} is NOT in any VMA")

    lines.append(f"")
    return '\n'.join(lines)


def main():
    import argparse
    parser = argparse.ArgumentParser(description='Parse Kevlar crash reports')
    parser.add_argument('logfile', nargs='?', help='Log file (stdin if omitted)')
    parser.add_argument('--binary', '-b', help='ELF binary for symbol resolution')
    parser.add_argument('--json', action='store_true', help='Output raw JSON')
    parser.add_argument('--pid', type=int, help='Filter by PID')
    args = parser.parse_args()

    if args.logfile:
        with open(args.logfile) as f:
            events = parse_events(f)
    else:
        events = parse_events(sys.stdin)

    crashes = find_crashes(events)
    if args.pid:
        crashes = [c for c in crashes if c['pid'] == args.pid]

    if not crashes:
        print("No crash reports found.", file=sys.stderr)
        sys.exit(1)

    if args.json:
        for c in crashes:
            print(json.dumps(c, indent=2))
    else:
        for c in crashes:
            print(format_crash(c, binary=args.binary))


if __name__ == '__main__':
    main()
