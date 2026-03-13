#!/usr/bin/env python3
"""
Syscall Trace Diff Tool — M6.5 Phase 1.5
==========================================
Runs a contract test binary under strace (Linux) and Kevlar (QEMU with
debug=syscall), then aligns the syscall sequences and reports the first
divergence.  This tool short-circuits the "read kernel source for an hour"
cycle by showing exactly which syscall returned the wrong value.

Usage:
    python3 tools/diff-syscall-traces.py TEST_STEM [options]

    TEST_STEM   Contract test stem, e.g. brk_basic, mprotect_basic

Options:
    --arch ARCH        x64 (default) or arm64
    --kernel PATH      Kevlar kernel ELF (auto-detect kevlar.x64.elf)
    --timeout N        QEMU timeout in seconds (default 30)
    --filter SYS,...   Only show specific syscall names (comma-separated)
    --no-linux         Skip Linux strace run
    --no-kevlar        Skip Kevlar QEMU run
    --build-dir DIR    Where compiled test binaries are (default: build/contracts)
    --cc CMD           C compiler (default: gcc)
    --context N        Lines of context around first divergence (default 5)
    --verbose          Show full aligned trace, not just divergence

Exit code: 0 = no divergence, 1 = divergence found, 2 = error.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# ---------------------------------------------------------------------------
# Syscall result normalisation
# ---------------------------------------------------------------------------

def result_class(ret: int, errno_name: str | None = None) -> str:
    """Classify a syscall return value as 'ok' or 'error:NAME'."""
    if ret < 0:
        return f"error:{errno_name or str(-ret)}"
    return "ok"

def result_detail(ret: int) -> str:
    """Human-readable return value for display (don't compare exact addresses)."""
    if ret < 0:
        return str(ret)
    if ret > 0x10000:
        return f"{ret:#x}"
    return str(ret)


# ---------------------------------------------------------------------------
# Linux strace capture
# ---------------------------------------------------------------------------

_STRACE_LINE_RE = re.compile(
    r'^(\w+)\s*\(.*\)\s*=\s*(-?\d+|0x[0-9a-fA-F]+)(?:\s+(\w+)\s+\(([^)]+)\))?'
)
_STRACE_UNFINISHED_RE = re.compile(r'^(\w+)\s*\(.*<unfinished')
_STRACE_RESUMED_RE = re.compile(r'^<\.\.\.\s*(\w+)\s+resumed>.*=\s*(-?\d+|0x[0-9a-fA-F]+)')

def parse_strace_output(text: str) -> list[dict]:
    """Parse strace -f output into a list of {name, ret, errno, raw} dicts."""
    calls = []
    pending: dict[str, str] = {}  # tid -> syscall name for <unfinished> pairs

    for line in text.splitlines():
        # Strip PID prefix if present (strace -f output: "12345 syscall(...)")
        line = re.sub(r'^\s*\d+\s+', '', line).strip()

        # Handle resumed syscalls
        m = _STRACE_RESUMED_RE.match(line)
        if m:
            name = m.group(1)
            ret_str = m.group(2)
            ret = int(ret_str, 16) if ret_str.startswith('0x') else int(ret_str)
            if ret > 0x7fffffffffffffff:
                ret = ret - (1 << 64)  # sign-extend
            calls.append({'name': name, 'ret': ret, 'errno': None, 'raw': line})
            continue

        m = _STRACE_LINE_RE.match(line)
        if not m:
            continue

        name = m.group(1)
        ret_str = m.group(2)
        ret = int(ret_str, 16) if ret_str.startswith('0x') else int(ret_str)
        if ret > 0x7fffffffffffffff:
            ret = ret - (1 << 64)  # sign-extend
        errno_name = m.group(3)  # e.g. "ENOSYS"
        calls.append({'name': name, 'ret': ret, 'errno': errno_name, 'raw': line})

    return calls


def run_linux_strace(binary: Path, timeout: int) -> list[dict] | None:
    """Run binary under strace. Returns parsed syscall list or None on error."""
    try:
        result = subprocess.run(
            ['strace', '-f', '-e', 'trace=all', str(binary)],
            capture_output=True, text=True, timeout=timeout,
        )
        # strace writes its output to stderr
        return parse_strace_output(result.stderr)
    except FileNotFoundError:
        print("ERROR: strace not found. Install with: sudo apt install strace")
        return None
    except subprocess.TimeoutExpired:
        print(f"ERROR: Linux strace timed out after {timeout}s")
        return None
    except Exception as e:
        print(f"ERROR running strace: {e}")
        return None


# ---------------------------------------------------------------------------
# Kevlar QEMU capture
# ---------------------------------------------------------------------------

_DBG_RE = re.compile(r'^DBG\s+(\{.+\})\s*$')

def parse_kevlar_output(text: str) -> list[dict]:
    """Extract syscall trace from Kevlar's JSONL debug output."""
    entries: dict[str, dict] = {}  # nr key -> pending entry event
    calls = []

    for line in text.splitlines():
        m = _DBG_RE.match(line.strip())
        if not m:
            continue
        try:
            obj = json.loads(m.group(1))
        except json.JSONDecodeError:
            continue

        t = obj.get('type', '')
        if t == 'syscall_entry':
            key = f"{obj.get('pid',0)}:{obj.get('nr',0)}"
            entries[key] = obj
        elif t == 'syscall_exit':
            nr = obj.get('nr', 0)
            pid = obj.get('pid', 0)
            key = f"{pid}:{nr}"
            entry = entries.pop(key, None)
            calls.append({
                'name': obj.get('name', '(unknown)'),
                'nr': nr,
                'pid': pid,
                'ret': obj.get('result', 0),
                'errno': obj.get('errno'),
                'args': entry.get('args') if entry else None,
                'raw': line.strip(),
            })

    return calls


def run_kevlar(init_bin: str, kernel_elf: Path, arch: str, timeout: int) -> list[dict] | None:
    """Run binary in Kevlar with debug=syscall. Returns parsed call list or None."""
    with tempfile.TemporaryDirectory() as tmpdir:
        kernel_path = str(kernel_elf)
        if arch == 'x64':
            elf_data = bytearray(kernel_elf.read_bytes())
            elf_data[18] = 0x03
            elf_data[19] = 0x00
            tmp_elf = Path(tmpdir) / 'kernel-patched.elf'
            tmp_elf.write_bytes(bytes(elf_data))
            kernel_path = str(tmp_elf)

        init_path = f'/bin/{init_bin}'
        if arch == 'x64':
            qemu_args = [
                'qemu-system-x86_64',
                '-m', '256',
                '-cpu', 'Icelake-Server',
                '-nographic', '-no-reboot',
                '-serial', 'mon:stdio',
                '-monitor', 'none',
                '-d', 'guest_errors',
                '-device', 'isa-debug-exit,iobase=0x501,iosize=2',
                '-kernel', kernel_path,
                '-append', f'pci=off debug=syscall init={init_path}',
            ]
        else:
            qemu_args = [
                'qemu-system-aarch64',
                '-machine', 'virt',
                '-cpu', 'cortex-a72',
                '-m', '256',
                '-nographic', '-no-reboot',
                '-serial', 'mon:stdio',
                '-monitor', 'none',
                '-d', 'guest_errors',
                '-kernel', kernel_path,
                '-append', f'debug=syscall init={init_path}',
            ]

        try:
            result = subprocess.run(
                qemu_args, capture_output=True, text=True, timeout=timeout,
            )
            return parse_kevlar_output(result.stdout)
        except subprocess.TimeoutExpired:
            print(f"ERROR: Kevlar QEMU timed out after {timeout}s")
            return None
        except Exception as e:
            print(f"ERROR running Kevlar QEMU: {e}")
            return None


# ---------------------------------------------------------------------------
# Compile helper (same logic as compare-contracts.py)
# ---------------------------------------------------------------------------

def compile_test(src: Path, out: Path, cc: str) -> bool:
    out.parent.mkdir(parents=True, exist_ok=True)
    cmd = [cc, str(src), '-o', str(out), '-static', '-O1', '-Wall', '-Wno-unused-result']
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"Compile error: {result.stderr.strip()}")
        return False
    return True


# ---------------------------------------------------------------------------
# Trace alignment and diff
# ---------------------------------------------------------------------------

# Syscalls to ignore when diffing (very common, not interesting for parity)
_SKIP_NAMES = frozenset({
    'clock_gettime', 'clock_getres',  # often vDSO
    'arch_prctl', 'set_tid_address', 'set_robust_list',  # startup boilerplate
    'prlimit64', 'getrlimit',  # resource limits
    'access', 'openat', 'read', 'close', 'fstat', 'newfstatat',  # ld boilerplate
    'mmap', 'munmap', 'mprotect',  # memory management (addresses differ)
    'futex',  # mutex/condvar
})


def align_traces(
    linux: list[dict],
    kevlar: list[dict],
    name_filter: set[str] | None,
) -> list[tuple[dict | None, dict | None]]:
    """
    Align two syscall sequences using a greedy forward scan.
    Returns list of (linux_entry, kevlar_entry) pairs.
    None means unmatched on that side.
    """
    # Filter to interesting syscalls
    def keep(c: dict) -> bool:
        if name_filter:
            return c['name'] in name_filter
        return c['name'] not in _SKIP_NAMES

    lf = [c for c in linux  if keep(c)]
    kf = [c for c in kevlar if keep(c)]

    pairs = []
    li, ki = 0, 0
    LOOKAHEAD = 4

    while li < len(lf) and ki < len(kf):
        lc = lf[li]
        kc = kf[ki]

        if lc['name'] == kc['name']:
            pairs.append((lc, kc))
            li += 1
            ki += 1
            continue

        # Names differ — try to resync within lookahead
        resynced = False
        for delta in range(1, LOOKAHEAD + 1):
            if li + delta < len(lf) and lf[li + delta]['name'] == kc['name']:
                for skip in range(delta):
                    pairs.append((lf[li + skip], None))
                li += delta
                resynced = True
                break
            if ki + delta < len(kf) and kf[ki + delta]['name'] == lc['name']:
                for skip in range(delta):
                    pairs.append((None, kf[ki + skip]))
                ki += delta
                resynced = True
                break

        if not resynced:
            pairs.append((lc, kc))
            li += 1
            ki += 1

    while li < len(lf):
        pairs.append((lf[li], None))
        li += 1
    while ki < len(kf):
        pairs.append((None, kf[ki]))
        ki += 1

    return pairs


def is_divergence(lc: dict | None, kc: dict | None) -> bool:
    """True if this pair represents a meaningful divergence."""
    if lc is None or kc is None:
        return True  # one side is missing the syscall entirely
    lclass = result_class(lc['ret'], lc.get('errno'))
    kclass = result_class(kc['ret'], kc.get('errno'))
    return lclass != kclass


def format_pair(lc: dict | None, kc: dict | None, diverges: bool) -> str:
    marker = '>>>' if diverges else '   '
    lname  = lc['name'] if lc else '(missing)'
    kname  = kc['name'] if kc else '(missing)'
    lret   = result_detail(lc['ret']) if lc else '-'
    kret   = result_detail(kc['ret']) if kc else '-'
    lerrno = (lc.get('errno') or '') if lc else ''
    kerrno = (kc.get('errno') or '') if kc else ''

    if lname == kname:
        name_col = f"{lname:20s}"
    else:
        name_col = f"linux={lname!r} kevlar={kname!r}"

    linux_col  = f"{lret:>12s}  {lerrno:<10s}" if lc else f"{'(none)':>12s}"
    kevlar_col = f"{kret:>12s}  {kerrno:<10s}" if kc else f"{'(none)':>12s}"
    return f"  {marker}  {name_col}  linux: {linux_col}  kevlar: {kevlar_col}"


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument('test_stem', help='Contract test stem (e.g. brk_basic)')
    parser.add_argument('--arch', choices=['x64', 'arm64'], default='x64')
    parser.add_argument('--kernel', metavar='PATH')
    parser.add_argument('--timeout', type=int, default=30)
    parser.add_argument('--filter', metavar='SYSCALLS',
                        help='Comma-separated syscall names to focus on')
    parser.add_argument('--no-linux', action='store_true')
    parser.add_argument('--no-kevlar', action='store_true')
    parser.add_argument('--build-dir', default='build/contracts')
    parser.add_argument('--cc', default='gcc')
    parser.add_argument('--context', type=int, default=5)
    parser.add_argument('--verbose', action='store_true')
    args = parser.parse_args()

    repo_root = Path(__file__).parent.parent
    contracts_dir = repo_root / 'testing' / 'contracts'
    build_dir = repo_root / args.build_dir

    # Find the source file
    matches = list(contracts_dir.rglob(f'{args.test_stem}.c'))
    if not matches:
        print(f"ERROR: No contract test named '{args.test_stem}.c' found under {contracts_dir}")
        return 2
    src = matches[0]

    # Compile (for Linux run)
    binary = build_dir / src.relative_to(contracts_dir).with_suffix('')
    if not args.no_linux:
        print(f"Compiling {src.name} ...")
        if not compile_test(src, binary, args.cc):
            return 2

    # Auto-detect kernel ELF
    if args.kernel:
        kernel_elf = Path(args.kernel)
    else:
        kernel_elf = repo_root / f'kevlar.{args.arch}.elf'
        if not kernel_elf.exists() and not args.no_kevlar:
            print(f"ERROR: kernel not found at {kernel_elf}. Build first or use --kernel.")
            return 2

    name_filter = set(args.filter.split(',')) if args.filter else None

    # --- Run Linux ---
    linux_calls: list[dict] = []
    if not args.no_linux:
        print(f"Running under strace (Linux) ...")
        t0 = time.monotonic()
        result = run_linux_strace(binary, args.timeout)
        if result is None:
            return 2
        linux_calls = result
        print(f"  Linux: {len(linux_calls)} syscalls captured ({time.monotonic()-t0:.1f}s)")

    # --- Run Kevlar ---
    kevlar_calls: list[dict] = []
    if not args.no_kevlar:
        init_bin = f'contract-{args.test_stem}'
        print(f"Running in Kevlar (QEMU, debug=syscall) ...")
        t0 = time.monotonic()
        result = run_kevlar(init_bin, kernel_elf, args.arch, args.timeout)
        if result is None:
            return 2
        kevlar_calls = result
        print(f"  Kevlar: {len(kevlar_calls)} syscalls captured ({time.monotonic()-t0:.1f}s)")

    if not linux_calls and not kevlar_calls:
        print("Nothing to compare (both --no-linux and --no-kevlar?)")
        return 2

    if args.no_linux or args.no_kevlar:
        side = 'Kevlar' if args.no_linux else 'Linux'
        calls = kevlar_calls if args.no_linux else linux_calls
        print(f"\n{side} syscall trace ({len(calls)} calls):")
        for c in calls:
            print(f"  {c['name']:20s}  {result_detail(c['ret']):>12s}  {c.get('errno') or ''}")
        return 0

    # --- Align and diff ---
    pairs = align_traces(linux_calls, kevlar_calls, name_filter)

    divergences = [i for i, (l, k) in enumerate(pairs) if is_divergence(l, k)]
    total_compared = len(pairs)
    n_div = len(divergences)

    print(f"\n{'─'*70}")
    print(f"Aligned {total_compared} syscall pairs.  Divergences: {n_div}")
    print(f"{'─'*70}")

    if n_div == 0:
        print("  No divergences found — Linux and Kevlar agree on all syscall results.")
        return 0

    # Show context around first divergence
    first_div = divergences[0]
    start = max(0, first_div - args.context)
    end   = min(len(pairs), first_div + args.context + 1)

    print(f"\nFirst divergence at pair #{first_div}  (showing ±{args.context} context)\n")
    print(f"  {'':5s}  {'syscall':20s}  {'Linux result':>25s}  {'Kevlar result':>25s}")
    print(f"  {'':5s}  {'─'*20}  {'─'*25}  {'─'*25}")

    if args.verbose:
        show_range = range(len(pairs))
    else:
        show_range = range(start, end)

    for i in show_range:
        lc, kc = pairs[i]
        div = is_divergence(lc, kc)
        line = format_pair(lc, kc, div)
        print(line)

    if n_div > 1 and not args.verbose:
        remaining = n_div - 1
        print(f"\n  ... {remaining} more divergence(s). Use --verbose to see all.")

    print(f"\n{'─'*70}")
    if divergences:
        first_l, first_k = pairs[first_div]
        name = (first_l or first_k or {}).get('name', '?')
        linux_ret  = result_detail(first_l['ret']) if first_l else '(none)'
        kevlar_ret = result_detail(first_k['ret']) if first_k else '(none)'
        linux_err  = first_l.get('errno') or '' if first_l else ''
        kevlar_err = first_k.get('errno') or '' if first_k else ''
        print(f"ROOT CAUSE CANDIDATE: {name}()")
        print(f"  Linux  → {linux_ret} {linux_err}")
        print(f"  Kevlar → {kevlar_ret} {kevlar_err}")
        print()

    return 1


if __name__ == '__main__':
    sys.exit(main())
