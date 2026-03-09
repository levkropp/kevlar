#!/usr/bin/env python3
"""
Cross-platform benchmark runner.

Builds Kevlar with /bin/bench as init, runs in QEMU, and extracts results.

Usage:
    python tools/run-bench.py [--arch x64|arm64] [--profile PROFILE]
    python tools/run-bench.py --quick  # Run quick benchmarks
"""

import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path


def find_qemu(arch="x64"):
    """Find QEMU executable."""
    qemu_bin = "qemu-system-x86_64" if arch == "x64" else "qemu-system-aarch64"

    # Try environment variable first
    if "QEMU_PATH" in os.environ and os.path.exists(os.environ["QEMU_PATH"]):
        return os.environ["QEMU_PATH"]

    # Try which/where
    tool = shutil.which(qemu_bin)
    if tool:
        return tool

    # Windows-specific paths
    if platform.system() == "Windows":
        common_paths = [
            rf"C:\Program Files\qemu\{qemu_bin}.exe",
            rf"C:\qemu\{qemu_bin}.exe",
        ]
        for path in common_paths:
            if os.path.exists(path):
                return path

    return None


def build_kernel_with_bench(arch="x64", profile="balanced", release=False):
    """Build kernel with /bin/bench as init script."""
    root_dir = Path(__file__).parent.parent

    # Set environment for build
    env = os.environ.copy()
    env["INIT_SCRIPT"] = "/bin/bench"
    env["ARCH"] = arch
    env["PROFILE"] = profile
    if release:
        env["RELEASE"] = "1"

    # Disable MSYS path conversion on Windows
    if platform.system() == "Windows":
        env["MSYS_NO_PATHCONV"] = "1"
        env["MSYS2_ARG_CONV_EXCL"] = "*"

    print(f"Building kernel with benchmark init script ({profile} profile)...", file=sys.stderr)

    # Call build.py
    build_script = root_dir / "tools" / "build.py"
    result = subprocess.run(
        [sys.executable, str(build_script), "build"],
        env=env,
        cwd=root_dir
    )

    if result.returncode != 0:
        print("Build failed", file=sys.stderr)
        return False

    return True


def run_benchmark(arch="x64", timeout=90):
    """Run QEMU with benchmark and capture output."""
    root_dir = Path(__file__).parent.parent
    kernel_elf = root_dir / f"kevlar.{arch}.elf"

    if not kernel_elf.exists():
        print(f"Error: {kernel_elf} not found", file=sys.stderr)
        return None

    # Find QEMU
    qemu = find_qemu(arch)
    if not qemu:
        print(f"Error: qemu-system-{arch} not found", file=sys.stderr)
        print("Install QEMU or set QEMU_PATH environment variable", file=sys.stderr)
        return None

    print(f"Running benchmark in QEMU (timeout: {timeout}s)...", file=sys.stderr)
    print(f"QEMU: {qemu}", file=sys.stderr)

    # Set up environment
    env = os.environ.copy()
    if platform.system() == "Windows":
        env["MSYS_NO_PATHCONV"] = "1"
        env["MSYS2_ARG_CONV_EXCL"] = "*"

    # Run QEMU via run-qemu.py
    run_qemu_script = root_dir / "tools" / "run-qemu.py"

    try:
        result = subprocess.run(
            [sys.executable, str(run_qemu_script), "--arch", arch, str(kernel_elf)],
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=root_dir
        )
        output = result.stdout + result.stderr
    except subprocess.TimeoutExpired as e:
        output = (e.stdout or "") + (e.stderr or "")
        print("QEMU timeout - collecting partial results", file=sys.stderr)

    return output


def parse_benchmark_results(output):
    """Parse BENCH lines from output."""
    results = {}

    # Look for BENCH lines
    # Format: BENCH test_name iterations total_ns per_iter_ns
    bench_pattern = re.compile(r'BENCH\s+(\S+)\s+(\d+)\s+(\d+)\s+(\d+)')

    for line in output.splitlines():
        # Remove ANSI color codes
        clean_line = re.sub(r'\x1b\[[0-9;]*m', '', line)

        match = bench_pattern.search(clean_line)
        if match:
            test_name, iterations, total_ns, per_iter_ns = match.groups()
            results[test_name] = {
                "iterations": int(iterations),
                "total_ns": int(total_ns),
                "per_iter_ns": int(per_iter_ns)
            }

    return results


def print_results(results, baseline=None):
    """Print benchmark results in a formatted table."""
    if not results:
        print("No benchmark results found", file=sys.stderr)
        return

    print("\n=== Benchmark Results ===\n")
    print(f"{'Test':<20} {'Iterations':>12} {'Per-iter (ns)':>15} {'vs Baseline':>15}")
    print("-" * 70)

    for test_name in sorted(results.keys()):
        data = results[test_name]
        per_iter = data["per_iter_ns"]
        iterations = data["iterations"]

        vs_baseline = ""
        if baseline and test_name in baseline:
            baseline_ns = baseline[test_name]["per_iter_ns"]
            ratio = per_iter / baseline_ns
            if ratio < 1:
                vs_baseline = f"{(1-ratio)*100:+.1f}% faster"
            else:
                vs_baseline = f"{(ratio-1)*100:+.1f}% slower"

        print(f"{test_name:<20} {iterations:>12,} {per_iter:>15,} {vs_baseline:>15}")

    print()


def main():
    parser = argparse.ArgumentParser(description="Run Kevlar benchmarks")
    parser.add_argument("--arch", default="x64", choices=["x64", "arm64"],
                       help="Target architecture (default: x64)")
    parser.add_argument("--profile", default="balanced",
                       choices=["fortress", "balanced", "performance", "ludicrous"],
                       help="Safety profile (default: balanced)")
    parser.add_argument("--release", action="store_true",
                       help="Build in release mode")
    parser.add_argument("--quick", action="store_true",
                       help="Run quick benchmarks (fewer iterations)")
    parser.add_argument("--timeout", type=int, default=90,
                       help="QEMU timeout in seconds (default: 90)")
    parser.add_argument("--no-build", action="store_true",
                       help="Skip build, just run existing kernel")

    args = parser.parse_args()

    # Build kernel
    if not args.no_build:
        if not build_kernel_with_bench(args.arch, args.profile, args.release):
            return 1

    # Run benchmark
    output = run_benchmark(args.arch, args.timeout)
    if output is None:
        return 1

    # Save full output for debugging
    output_file = Path("benchmark-output.txt")
    with open(output_file, "w") as f:
        f.write(output)
    print(f"Full output saved to: {output_file}", file=sys.stderr)

    # Parse and display results
    results = parse_benchmark_results(output)
    print_results(results)

    if not results:
        print("\nERROR: No benchmark results found!", file=sys.stderr)
        print("Check benchmark-output.txt for details", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
