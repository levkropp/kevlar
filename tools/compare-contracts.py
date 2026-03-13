#!/usr/bin/env python3
"""
M6.5 Contract Test Harness
===========================
Compiles each C contract test statically, runs it on the host (Linux
baseline) and in Kevlar (QEMU), then compares output.

Usage:
    python3 tools/compare-contracts.py [options] [FILTER...]

    FILTER  Substring match against test path, e.g. "vm/fork_cow"

Options:
    --arch ARCH        x64 (default) or arm64
    --timeout N        Per-test QEMU timeout in seconds (default 30)
    --json FILE        Write full results to JSON file
    --no-linux         Skip Linux baseline (useful if cross-compiling)
    --no-kevlar        Skip Kevlar run (just verify Linux baseline)
    --kernel PATH      Path to Kevlar kernel ELF (default: auto-detect)
    --cc CMD           C compiler command (default: gcc)
    --cflags FLAGS     Extra CFLAGS (space-separated)
    --build-dir DIR    Where to put compiled test binaries (default: build/contracts)
    --verbose          Print full test output even on PASS

Exit code: 0 if all tests pass, 1 if any DIVERGE or FAIL.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# Marker printed by contract tests to signal pass/fail.
PASS_MARKER = "CONTRACT_PASS"
FAIL_MARKER = "CONTRACT_FAIL"

# -------------------------------------------------------------------
# Compiler
# -------------------------------------------------------------------

def compile_test(src: Path, out: Path, cc: str, extra_cflags: list[str]) -> tuple[bool, str]:
    """Compile src to a static binary at out. Returns (ok, error_msg)."""
    out.parent.mkdir(parents=True, exist_ok=True)
    cmd = [cc, str(src), "-o", str(out), "-static", "-O1",
           "-Wall", "-Wno-unused-result"] + extra_cflags
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        return False, result.stderr.strip()
    return True, ""


# -------------------------------------------------------------------
# Linux run (native)
# -------------------------------------------------------------------

def run_linux(binary: Path, timeout: int) -> tuple[str, bool]:
    """Run binary natively. Returns (output, timed_out)."""
    try:
        result = subprocess.run(
            [str(binary)],
            capture_output=True, text=True,
            timeout=timeout,
        )
        return result.stdout + result.stderr, False
    except subprocess.TimeoutExpired:
        return "", True
    except Exception as e:
        return f"ERROR: {e}", False


# -------------------------------------------------------------------
# Kevlar run (QEMU)
# -------------------------------------------------------------------

def kevlar_bin_name(src: Path, contracts_dir: Path) -> str:
    """Map a contract test source path to its binary name in Kevlar's initramfs.
    e.g. testing/contracts/vm/fork_cow.c -> contract-fork_cow"""
    stem = src.stem  # e.g. "fork_cow"
    return f"contract-{stem}"


def run_kevlar(init_bin: str, kernel_elf: Path, arch: str, timeout: int) -> tuple[str, bool]:
    """Run a contract test binary (by /bin/<name>) in Kevlar via QEMU.
    The binary must already be present in the kernel's embedded initramfs.
    Returns (serial_output, timed_out)."""
    with tempfile.TemporaryDirectory() as tmpdir:
        # Patch kernel ELF for x64: e_machine must be EM_386 for QEMU multiboot
        kernel_path = str(kernel_elf)
        if arch == "x64":
            elf_data = bytearray(kernel_elf.read_bytes())
            elf_data[18] = 0x03  # EM_386 low byte
            elf_data[19] = 0x00  # EM_386 high byte
            tmp_elf = Path(tmpdir) / "kernel-patched.elf"
            tmp_elf.write_bytes(bytes(elf_data))
            kernel_path = str(tmp_elf)

        # Use init= cmdline to override the compiled-in INIT_SCRIPT.
        # The contract binary is in Kevlar's embedded initramfs at /bin/<init_bin>.
        init_path = f"/bin/{init_bin}"

        if arch == "x64":
            qemu_bin = "qemu-system-x86_64"
            qemu_args = [
                "-m", "256",
                "-cpu", "Icelake-Server",
                "-nographic",
                "-no-reboot",
                "-serial", "mon:stdio",
                "-monitor", "none",
                "-d", "guest_errors",
                # isa-debug-exit: Kevlar writes to port 0x501 to exit QEMU
                "-device", "isa-debug-exit,iobase=0x501,iosize=2",
                "-kernel", kernel_path,
                "-append", f"pci=off init={init_path}",
            ]
        else:
            qemu_bin = "qemu-system-aarch64"
            qemu_args = [
                "-machine", "virt",
                "-cpu", "cortex-a72",
                "-m", "256",
                "-nographic",
                "-no-reboot",
                "-serial", "mon:stdio",
                "-monitor", "none",
                "-d", "guest_errors",
                "-kernel", kernel_path,
                "-append", f"init={init_path}",
            ]

        try:
            result = subprocess.run(
                [qemu_bin] + qemu_args,
                capture_output=True, text=True,
                timeout=timeout,
            )
            return result.stdout, False
        except subprocess.TimeoutExpired:
            return "", True
        except Exception as e:
            return f"ERROR: {e}", False


# -------------------------------------------------------------------
# Compare outputs
# -------------------------------------------------------------------

import re as _re
_ANSI_RE = _re.compile(r'\x1b\[[0-9;?]*[a-zA-Z]|\x1b\[[\x00-\x1f]*')

# Prefixes of lines that are kernel/QEMU noise, not contract test output.
_NOISE_PREFIXES = (
    "SeaBIOS",
    "iPXE",
    "Booting from",
    "Press Ctrl",
    "QEMU",
    "qemu",
    "[    ",     # Linux kernel dmesg style
)

def extract_contract_lines(output: str) -> list[str]:
    """Return only the contract test output lines, stripping kernel/QEMU noise."""
    lines = []
    for raw in output.splitlines():
        # Strip ANSI escape sequences and surrounding whitespace
        line = _ANSI_RE.sub("", raw).strip()
        if not line:
            continue
        # Drop kernel log lines: they start with a color code stripped to nothing
        # or contain Kevlar log-level markers
        if any(line.startswith(p) for p in _NOISE_PREFIXES):
            continue
        # Kevlar log lines: "cmdline: ...", "bootinfo: ...", "available RAM:", etc.
        # They don't contain ':' at a reasonable position for contract output, but
        # the safest filter is: if the raw line had a leading ANSI code it's a
        # kernel log line.
        raw_stripped = raw.strip()
        if raw_stripped and raw_stripped[0] == '\x1b':
            continue
        lines.append(line)
    return lines


def compare(linux_out: str, kevlar_out: str) -> tuple[str, list[str]]:
    """
    Returns (status, issues) where status is PASS, DIVERGE, or FAIL.
    PASS: both outputs contain CONTRACT_PASS and no CONTRACT_FAIL.
    FAIL: CONTRACT_FAIL in either output.
    DIVERGE: outputs differ (or one lacks CONTRACT_PASS).
    """
    linux_lines = extract_contract_lines(linux_out)
    kevlar_lines = extract_contract_lines(kevlar_out)

    linux_pass = any(PASS_MARKER in l for l in linux_lines)
    kevlar_pass = any(PASS_MARKER in l for l in kevlar_lines)
    linux_fail = any(FAIL_MARKER in l for l in linux_lines)
    kevlar_fail = any(FAIL_MARKER in l for l in kevlar_lines)

    issues = []
    if linux_fail:
        issues.append("Linux reported CONTRACT_FAIL")
    if kevlar_fail:
        issues.append("Kevlar reported CONTRACT_FAIL")
    if not linux_pass and not linux_fail:
        issues.append("Linux: no CONTRACT_PASS marker found")
    if not kevlar_pass and not kevlar_fail:
        issues.append("Kevlar: no CONTRACT_PASS marker found")
    if linux_lines != kevlar_lines:
        issues.append("output differs")

    if linux_fail or kevlar_fail:
        return "FAIL", issues
    if not linux_pass or not kevlar_pass or linux_lines != kevlar_lines:
        return "DIVERGE", issues
    return "PASS", []


# -------------------------------------------------------------------
# Discovery
# -------------------------------------------------------------------

def find_tests(root: Path, filters: list[str]) -> list[Path]:
    tests = sorted(root.rglob("*.c"))
    if filters:
        tests = [t for t in tests if any(f in str(t) for f in filters)]
    return tests


# -------------------------------------------------------------------
# Main
# -------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("filters", nargs="*", help="Substring filter for test paths")
    parser.add_argument("--arch", choices=["x64", "arm64"], default="x64")
    parser.add_argument("--timeout", type=int, default=30)
    parser.add_argument("--json", metavar="FILE")
    parser.add_argument("--no-linux", action="store_true")
    parser.add_argument("--no-kevlar", action="store_true")
    parser.add_argument("--kernel", metavar="PATH")
    parser.add_argument("--cc", default="gcc")
    parser.add_argument("--cflags", default="")
    parser.add_argument("--build-dir", default="build/contracts")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()

    repo_root = Path(__file__).parent.parent
    contracts_dir = repo_root / "testing" / "contracts"
    build_dir = repo_root / args.build_dir

    # Auto-detect kernel
    if args.kernel:
        kernel_elf = Path(args.kernel)
    else:
        default_elf = repo_root / f"kevlar.{args.arch}.elf"
        if not default_elf.exists():
            print(f"ERROR: kernel not found at {default_elf}. Build first or use --kernel.")
            sys.exit(1)
        kernel_elf = default_elf

    tests = find_tests(contracts_dir, args.filters)
    if not tests:
        print(f"No tests found in {contracts_dir}" +
              (f" matching {args.filters}" if args.filters else ""))
        sys.exit(1)

    extra_cflags = args.cflags.split() if args.cflags else []

    results = []
    passed = failed = diverged = skipped = 0

    print(f"Running {len(tests)} contract test(s) [arch={args.arch}]")
    print()

    for src in tests:
        rel = src.relative_to(contracts_dir)
        test_name = str(rel).replace(".c", "").replace("/", ".")
        binary = build_dir / rel.with_suffix("")

        # Compile
        ok, err = compile_test(src, binary, args.cc, extra_cflags)
        if not ok:
            print(f"  SKIP  {test_name}  (compile error: {err[:60]})")
            results.append({"test": test_name, "status": "SKIP", "compile_error": err})
            skipped += 1
            continue

        # Linux run
        linux_out = ""
        linux_timeout = False
        if not args.no_linux:
            t0 = time.monotonic()
            linux_out, linux_timeout = run_linux(binary, args.timeout)
            linux_time = time.monotonic() - t0
        else:
            linux_time = 0.0

        # Kevlar run
        kevlar_out = ""
        kevlar_timeout = False
        if not args.no_kevlar:
            init_bin = kevlar_bin_name(src, contracts_dir)
            t0 = time.monotonic()
            kevlar_out, kevlar_timeout = run_kevlar(init_bin, kernel_elf, args.arch, args.timeout)
            kevlar_time = time.monotonic() - t0
        else:
            kevlar_time = 0.0

        if linux_timeout:
            linux_out = "TIMEOUT"
        if kevlar_timeout:
            kevlar_out = "TIMEOUT"

        if args.no_kevlar:
            # Linux-only mode: just check for CONTRACT_PASS
            linux_lines = extract_contract_lines(linux_out)
            if any(PASS_MARKER in l for l in linux_lines):
                status, issues = "PASS", []
            else:
                status, issues = "FAIL", ["no CONTRACT_PASS on Linux"]
        elif args.no_linux:
            kevlar_lines = extract_contract_lines(kevlar_out)
            if any(PASS_MARKER in l for l in kevlar_lines):
                status, issues = "PASS", []
            else:
                status, issues = "FAIL", ["no CONTRACT_PASS on Kevlar"]
        else:
            status, issues = compare(linux_out, kevlar_out)

        if status == "PASS":
            passed += 1
            marker = "  PASS "
        elif status == "FAIL":
            failed += 1
            marker = "  FAIL "
        else:
            diverged += 1
            marker = "  DIVG "

        timing = ""
        if not args.no_linux and not args.no_kevlar:
            timing = f"  (linux={linux_time:.1f}s  kevlar={kevlar_time:.1f}s)"

        print(f"{marker} {test_name}{timing}")

        if issues or args.verbose:
            for issue in issues:
                print(f"         issue: {issue}")

        if args.verbose or status != "PASS":
            if linux_out and not args.no_linux:
                print("    --- Linux output ---")
                for l in extract_contract_lines(linux_out):
                    print(f"    {l}")
            if kevlar_out and not args.no_kevlar:
                print("    --- Kevlar output ---")
                for l in extract_contract_lines(kevlar_out):
                    print(f"    {l}")

        results.append({
            "test": test_name,
            "status": status,
            "issues": issues,
            "linux_output": linux_out,
            "kevlar_output": kevlar_out,
            "linux_timeout": linux_timeout,
            "kevlar_timeout": kevlar_timeout,
        })

    print()
    total = passed + failed + diverged + skipped
    print(f"Results: {passed}/{total} PASS  |  {diverged} DIVERGE  |  {failed} FAIL  |  {skipped} SKIP")

    if args.json:
        out = {
            "arch": args.arch,
            "kernel": str(kernel_elf),
            "summary": {
                "total": total,
                "passed": passed,
                "diverged": diverged,
                "failed": failed,
                "skipped": skipped,
            },
            "results": results,
        }
        Path(args.json).parent.mkdir(parents=True, exist_ok=True)
        Path(args.json).write_text(json.dumps(out, indent=2))
        print(f"Results written to {args.json}")

    sys.exit(0 if (failed == 0 and diverged == 0) else 1)


if __name__ == "__main__":
    main()
