#!/usr/bin/env python3
"""
Kevlar comprehensive benchmark runner.

Runs benchmarks across all configurations and generates comparison tables:
  - 4 Kevlar profiles (fortress, balanced, performance, ludicrous) under KVM
  - Linux under KVM
  - Native Linux

Usage:
  python tools/run-all-benchmarks.py                          # Run all
  python tools/run-all-benchmarks.py --kevlar                 # Kevlar profiles only
  python tools/run-all-benchmarks.py --kevlar --profile balanced  # Single profile
  python tools/run-all-benchmarks.py --linux                  # Linux KVM + native only
  python tools/run-all-benchmarks.py --quick                  # Quick mode
  python tools/run-all-benchmarks.py --filter core            # Core benchmarks only
"""

import argparse
import csv
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROFILES = ["fortress", "balanced", "performance", "ludicrous"]
LINUX_KERNEL = Path("/lib/modules") / os.uname().release / "vmlinuz"
QEMU = "qemu-system-x86_64"
BENCH_TIMEOUT = 180  # seconds


def log(msg):
    print(f"\033[1;96m>>>\033[0m {msg}", flush=True)


def warn(msg):
    print(f"\033[1;33mWARN:\033[0m {msg}", flush=True)


def dim(msg):
    print(f"    \033[2m{msg}\033[0m", flush=True)


def parse_bench_output(output):
    """Parse BENCH lines into {name: ns_per_iter}."""
    results = {}
    for line in output.split("\n"):
        m = re.match(r"BENCH\s+(\S+)\s+\d+\s+\d+\s+(\d+)", line)
        if m:
            results[m.group(1)] = int(m.group(2))
    return results


def run_streaming(cmd, timeout=BENCH_TIMEOUT, cwd=None, label=""):
    """Run a command, streaming BENCH lines to stdout in real-time.
    Returns the full collected output."""
    lines = []
    bench_count = 0
    start = time.monotonic()

    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        cwd=cwd,
    )

    try:
        for line in proc.stdout:
            line = line.rstrip("\n")
            lines.append(line)
            elapsed = time.monotonic() - start

            if line.startswith("BENCH ") and not line.startswith("BENCH_"):
                bench_count += 1
                # Parse and show result inline
                m = re.match(r"BENCH\s+(\S+)\s+\d+\s+\d+\s+(\d+)", line)
                if m:
                    dim(f"  {m.group(1)}: {fmt_ns(int(m.group(2)))}  [{elapsed:.1f}s]")
            elif line.startswith("BENCH_START"):
                dim(f"  VM booted, benchmarks starting...  [{elapsed:.1f}s]")
            elif line.startswith("BENCH_END"):
                dim(f"  Done ({bench_count} benchmarks)  [{elapsed:.1f}s]")
            elif line.startswith("BENCH_SKIP"):
                name = line.split()[1] if len(line.split()) > 1 else "?"
                dim(f"  SKIP {name}  [{elapsed:.1f}s]")

            if elapsed > timeout:
                warn(f"Timeout after {timeout}s, killing process")
                proc.kill()
                break
    except Exception as e:
        warn(f"Error reading output: {e}")
    finally:
        proc.wait(timeout=10)

    if bench_count == 0 and lines:
        # Show last few lines for debugging
        warn(f"No BENCH output received. Last 5 lines:")
        for l in lines[-5:]:
            dim(l)

    return "\n".join(lines)


def run_build(cmd, label="Building", cwd=None, timeout=300):
    """Run a build command with progress indication."""
    log(f"{label}...")
    start = time.monotonic()
    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=timeout,
        cwd=cwd,
    )
    elapsed = time.monotonic() - start
    if result.returncode != 0:
        warn(f"Build failed ({elapsed:.1f}s):")
        for line in result.stderr.strip().split("\n")[-10:]:
            dim(line)
        return False
    dim(f"Build OK ({elapsed:.1f}s)")
    return True


def run_kevlar_profile(profile, bench_filter="all", quick=False):
    """Build and run Kevlar with a specific profile under KVM."""
    init_script = "/bin/bench --full" if not quick else "/bin/bench"
    if bench_filter != "all":
        init_script += f" {bench_filter}"

    if not run_build(
        ["make", "build", f"PROFILE={profile}", f"INIT_SCRIPT={init_script}"],
        label=f"Building Kevlar (profile={profile})",
        cwd=ROOT,
    ):
        return {}

    kernel_elf = ROOT / "kevlar.x64.elf"
    if not kernel_elf.exists():
        warn(f"Kernel ELF not found: {kernel_elf}")
        return {}

    log(f"Running Kevlar KVM (profile={profile})...")
    run_qemu_py = ROOT / "tools" / "run-qemu.py"
    cmd = [
        sys.executable, str(run_qemu_py),
        "--kvm", "--arch", "x64",
        str(kernel_elf),
        "--", "-mem-prealloc",
    ]
    output = run_streaming(cmd, timeout=BENCH_TIMEOUT, cwd=ROOT, label=profile)

    results = parse_bench_output(output)
    if results:
        log(f"  {profile}: {len(results)} benchmarks collected")
    return results


def run_linux_kvm(bench_filter="all", quick=False):
    """Build bench, create initramfs, run Linux under KVM."""
    if not LINUX_KERNEL.exists():
        warn(f"Linux kernel not found: {LINUX_KERNEL}")
        return {}

    bench_linux = ROOT / "build" / "bench.linux"
    (ROOT / "build").mkdir(exist_ok=True)

    if not run_build(
        ["gcc", "-static", "-O2", "-o", str(bench_linux),
         str(ROOT / "benchmarks" / "bench.c")],
        label="Compiling bench.c for Linux KVM",
    ):
        return {}

    # bench binary IS /init — it detects PID 1 and calls init_setup().
    # Pass --full via rdinit args for consistent comparison with Kevlar.
    log("Creating Linux initramfs...")
    with tempfile.TemporaryDirectory() as tmpdir:
        initdir = os.path.join(tmpdir, "rootfs")
        for d in ["dev", "proc", "sys", "tmp"]:
            os.makedirs(f"{initdir}/{d}")
        shutil.copy2(str(bench_linux), f"{initdir}/init")
        os.chmod(f"{initdir}/init", 0o755)

        initramfs = str((ROOT / "build" / "linux-bench.initramfs.gz").resolve())
        result = subprocess.run(
            f"cd {initdir} && find . -print0 | cpio --null -o --format=newc 2>/dev/null | gzip > {initramfs}",
            shell=True, capture_output=True,
        )
        if result.returncode != 0:
            warn("Failed to create initramfs")
            return {}

    log("Running Linux KVM...")
    cmd = [
        QEMU,
        "-kernel", str(LINUX_KERNEL),
        "-initrd", initramfs,
        "-append", "console=ttyS0 quiet panic=-1 rdinit=/init -- --full",
        "-m", "1024",
        "-nographic",
        "-no-reboot",
        "-cpu", "host",
        "--enable-kvm",
        "-mem-prealloc",
    ]
    output = run_streaming(cmd, timeout=BENCH_TIMEOUT, label="linux_kvm")

    results = parse_bench_output(output)
    if results:
        log(f"  Linux KVM: {len(results)} benchmarks collected")
    return results


def run_native(bench_filter="all"):
    """Compile and run bench natively."""
    bench_native = ROOT / "build" / "bench.native"
    (ROOT / "build").mkdir(exist_ok=True)

    if not run_build(
        ["gcc", "-static", "-O2", "-o", str(bench_native),
         str(ROOT / "benchmarks" / "bench.c")],
        label="Compiling bench.c natively",
    ):
        return {}

    log("Running native benchmark...")
    cmd = [str(bench_native), "--full"]
    if bench_filter != "all":
        cmd.append(bench_filter)

    output = run_streaming(cmd, timeout=BENCH_TIMEOUT, label="native")

    results = parse_bench_output(output)
    if results:
        log(f"  Native: {len(results)} benchmarks collected")
    return results


def fmt_ns(ns):
    """Format nanoseconds for display."""
    if ns is None or ns == 0:
        return "-"
    if ns >= 1_000_000:
        return f"{ns/1000:.0f}us"
    if ns >= 10_000:
        return f"{ns/1000:.1f}us"
    return f"{ns}ns"


def ratio_str(kevlar_ns, ref_ns):
    """Format ratio string."""
    if not ref_ns or not kevlar_ns:
        return "-"
    r = kevlar_ns / ref_ns
    return f"{r:.2f}x"


def print_table(all_results, bench_filter="all"):
    """Print formatted comparison table."""
    all_tests = set()
    for results in all_results.values():
        all_tests.update(results.keys())

    if not all_tests:
        print("\nNo benchmark results collected!")
        return

    core_order = [
        "getpid", "read_null", "write_null", "pipe", "fork_exit",
        "open_close", "mmap_fault", "stat"
    ]
    extended_order = [
        "gettid", "clock_gettime", "uname", "dup_close", "fcntl_getfl",
        "fstat", "lseek", "getcwd", "readlink", "access",
        "mmap_munmap", "mprotect", "brk", "sigaction", "sigprocmask",
        "pread", "writev"
    ]

    ordered = [t for t in core_order if t in all_tests]
    ext = [t for t in extended_order if t in all_tests]
    remaining = sorted(all_tests - set(ordered) - set(ext))
    ordered_ext = ext + remaining

    # Column keys in display order
    columns = []
    for p in PROFILES:
        key = f"kevlar_{p}"
        if key in all_results and all_results[key]:
            columns.append((key, p[:4].title()))  # Fort, Bala, Perf, Ludi
    if "linux_kvm" in all_results and all_results["linux_kvm"]:
        columns.append(("linux_kvm", "LinuxKVM"))
    if "native" in all_results and all_results["native"]:
        columns.append(("native", "Native"))

    if not columns:
        print("\nNo results to display!")
        return

    col_w = 10
    name_w = 16

    print()
    print("=" * (name_w + len(columns) * (col_w + 1)))
    print("  KEVLAR BENCHMARK RESULTS  (ns/op, lower is better)")
    print("=" * (name_w + len(columns) * (col_w + 1)))

    header = f"{'Benchmark':<{name_w}}"
    for _, label in columns:
        header += f" {label:>{col_w}}"
    print(header)
    print("-" * len(header))

    def print_section(tests, section_name):
        if not tests:
            return
        print(f"\n  {section_name}:")
        for test in tests:
            row = f"  {test:<{name_w - 2}}"
            for key, _ in columns:
                val = all_results.get(key, {}).get(test, 0)
                row += f" {fmt_ns(val):>{col_w}}"
            print(row)

    print_section(ordered, "Core Syscalls")
    print_section(ordered_ext, "Extended Syscalls")

    # Ratio table: Kevlar profiles vs Linux KVM
    linux_kvm = all_results.get("linux_kvm", {})
    if linux_kvm:
        kevlar_cols = [(k, l) for k, l in columns if k.startswith("kevlar_")]
        if kevlar_cols:
            print()
            w = name_w + len(kevlar_cols) * (col_w + 1)
            print("=" * w)
            print("  RATIO vs Linux KVM  (1.00x = same, lower = faster)")
            print("=" * w)

            header = f"{'Benchmark':<{name_w}}"
            for _, label in kevlar_cols:
                header += f" {label:>{col_w}}"
            print(header)
            print("-" * len(header))

            for section_name, tests in [("Core", ordered), ("Extended", ordered_ext)]:
                if not tests:
                    continue
                print(f"\n  {section_name}:")
                for test in tests:
                    ref = linux_kvm.get(test, 0)
                    row = f"  {test:<{name_w - 2}}"
                    for key, _ in kevlar_cols:
                        val = all_results.get(key, {}).get(test, 0)
                        row += f" {ratio_str(val, ref):>{col_w}}"
                    print(row)

    print()


def save_results(all_results):
    """Save raw results as JSON and CSV."""
    ts = time.strftime("%Y-%m-%dT%H:%M:%S")

    # JSON
    json_path = ROOT / "build" / "benchmark-results.json"
    data = {"timestamp": ts, "kernel": os.uname().release, "results": all_results}
    with open(json_path, "w") as f:
        json.dump(data, f, indent=2)

    # CSV
    csv_path = ROOT / "build" / "benchmark-results.csv"
    all_tests = set()
    for results in all_results.values():
        all_tests.update(results.keys())

    col_keys = sorted(all_results.keys())
    with open(csv_path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["benchmark"] + col_keys)
        for test in sorted(all_tests):
            row = [test] + [all_results[k].get(test, "") for k in col_keys]
            w.writerow(row)

    log(f"Results saved to {json_path} and {csv_path}")


def main():
    parser = argparse.ArgumentParser(description="Kevlar comprehensive benchmark runner")
    parser.add_argument("--kevlar", action="store_true", help="Run Kevlar profiles only")
    parser.add_argument("--linux", action="store_true", help="Run Linux KVM + native only")
    parser.add_argument("--quick", action="store_true", help="Quick mode (fewer iterations)")
    parser.add_argument("--filter", default="all",
                        help="Benchmark filter: all, core, extended, or comma-separated "
                             "test names (e.g. mmap_fault,clock_gettime,sigprocmask)")
    parser.add_argument("--profile",
                        help="Run a single Kevlar profile (e.g., balanced)")
    args = parser.parse_args()

    os.chdir(ROOT)

    run_kev = not args.linux
    run_lin = not args.kevlar

    all_results = {}

    if run_kev:
        profiles = [args.profile] if args.profile else PROFILES
        for profile in profiles:
            all_results[f"kevlar_{profile}"] = run_kevlar_profile(
                profile, args.filter, args.quick)

    if run_lin:
        all_results["linux_kvm"] = run_linux_kvm(args.filter, args.quick)
        all_results["native"] = run_native(args.filter)

    print_table(all_results, args.filter)
    save_results(all_results)
    return 0


if __name__ == "__main__":
    sys.exit(main())
