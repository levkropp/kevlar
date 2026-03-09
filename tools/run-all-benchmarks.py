#!/usr/bin/env python3
"""
Run benchmarks on all three environments and compare results:
1. Kevlar on QEMU TCG
2. Linux on QEMU TCG
3. Native Linux (WSL)
"""

import platform
import re
import subprocess
import sys
from pathlib import Path


def run_command(cmd, description, timeout=120000):
    """Run a command and return output."""
    print(f"Running: {description}...", flush=True)
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout / 1000,
            shell=True if isinstance(cmd, str) else False
        )
        output = result.stdout + result.stderr
        # Debug: show if we got BENCH output
        if "BENCH" in output:
            bench_lines = [line for line in output.split('\n') if 'BENCH' in line]
            print(f"  Found {len(bench_lines)} BENCH lines")
        return output
    except subprocess.TimeoutExpired as e:
        print(f"  TIMEOUT after {timeout/1000}s")
        # Return partial output if available
        return (e.stdout or "") + (e.stderr or "")
    except Exception as e:
        print(f"  ERROR: {e}")
        return f"ERROR: {e}"


def parse_bench_output(output):
    """Parse BENCH output format into a dict."""
    results = {}
    for line in output.split('\n'):
        match = re.match(r'BENCH\s+(\w+)\s+\d+\s+\d+\s+(\d+)', line)
        if match:
            test_name = match.group(1)
            ns_per_iter = int(match.group(2))
            results[test_name] = ns_per_iter
    return results


def run_kevlar_bench():
    """Run Kevlar benchmark."""
    print("\n" + "="*70)
    print("KEVLAR ON QEMU TCG")
    print("="*70)

    # Build Kevlar with benchmark
    print("Building Kevlar with benchmark as init...")
    result = run_command(
        "make build INIT_SCRIPT=/bin/bench",
        "Building Kevlar"
    )

    # Run benchmark
    output = run_command(
        ["python", "tools/run-bench.py"],
        "Running Kevlar benchmark",
        timeout=120000
    )

    return parse_bench_output(output)


def run_linux_qemu_bench():
    """Run Linux on QEMU TCG."""
    print("\n" + "="*70)
    print("LINUX ON QEMU TCG")
    print("="*70)

    # Check if kernel and initramfs exist
    if not Path("build/vmlinuz-bench").exists():
        print("ERROR: Linux kernel not built. Run 'python tools/build-linux-kernel.py' first.")
        return {}

    # Build benchmark binary for Linux
    print("Building benchmark for Linux...")
    if platform.system() == "Windows":
        # Convert Windows path to WSL path
        cwd = Path.cwd().resolve()
        wsl_path = str(cwd).replace('\\', '/').replace('C:/', '/mnt/c/').replace('D:/', '/mnt/d/').replace('E:/', '/mnt/e/')
        subprocess.run(
            ["wsl", "-u", "root", "bash", "-c",
             f"cd {wsl_path} && gcc -static -O2 -o build/bench.linux benchmarks/bench.c"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL
        )
    else:
        subprocess.run(
            ["gcc", "-static", "-O2", "-o", "build/bench.linux", "benchmarks/bench.c"],
            check=True
        )

    # Create initramfs
    print("Creating initramfs...")
    subprocess.run(
        ["python", "tools/make-initramfs.py", "build/bench.linux", "build/bench.initramfs.gz"],
        check=True,
        stdout=subprocess.DEVNULL
    )

    # Run in QEMU
    print("Running Linux benchmark in QEMU TCG...")
    qemu_path = "C:/Program Files/qemu/qemu-system-x86_64.exe" if platform.system() == "Windows" else "qemu-system-x86_64"

    output = run_command(
        [qemu_path, "-kernel", "build/vmlinuz-bench",
         "-initrd", "build/bench.initramfs.gz",
         "-append", "console=ttyS0 quiet",
         "-m", "1024", "-cpu", "qemu64",
         "-nographic", "-no-reboot"],
        "Running Linux in QEMU",
        timeout=120000
    )

    return parse_bench_output(output)


def run_native_linux_bench():
    """Run benchmark natively on Linux (WSL on Windows)."""
    print("\n" + "="*70)
    print("NATIVE LINUX (WSL)")
    print("="*70)

    if platform.system() == "Windows":
        # Build and run in WSL
        print("Building and running benchmark in WSL...")
        cwd = Path.cwd().resolve()
        wsl_path = str(cwd).replace('\\', '/').replace('C:/', '/mnt/c/').replace('D:/', '/mnt/d/').replace('E:/', '/mnt/e/')
        output = run_command(
            ["wsl", "-u", "root", "bash", "-c",
             f"cd {wsl_path} && "
             "gcc -O2 -o build/bench.native benchmarks/bench.c && "
             "./build/bench.native"],
            "Running native Linux benchmark"
        )
    else:
        # Native Linux
        print("Building and running benchmark natively...")
        subprocess.run(
            ["gcc", "-O2", "-o", "build/bench.native", "benchmarks/bench.c"],
            check=True
        )
        output = run_command(
            ["./build/bench.native"],
            "Running native benchmark"
        )

    return parse_bench_output(output)


def print_comparison_table(kevlar, linux_qemu, native):
    """Print a comparison table of all results."""
    print("\n" + "="*70)
    print("BENCHMARK COMPARISON")
    print("="*70)
    print()

    # Get all test names
    all_tests = set(kevlar.keys()) | set(linux_qemu.keys()) | set(native.keys())

    if not all_tests:
        print("ERROR: No benchmark results found!")
        return

    # Print header
    print(f"{'Test':<20} {'Kevlar TCG':>12} {'Linux TCG':>12} {'Native':>12} {'K/L Ratio':>10} {'Verdict':>15}")
    print("-" * 95)

    for test in sorted(all_tests):
        k_val = kevlar.get(test, 0)
        l_val = linux_qemu.get(test, 0)
        n_val = native.get(test, 0)

        # Calculate ratio (Kevlar vs Linux QEMU)
        if l_val > 0:
            ratio = k_val / l_val
            ratio_str = f"{ratio:.2f}x"

            # Verdict
            if ratio <= 1.10:
                verdict = "EXCELLENT"
            elif ratio <= 1.25:
                verdict = "GOOD"
            elif ratio <= 1.50:
                verdict = "OK"
            else:
                verdict = "SLOW"
        else:
            ratio_str = "N/A"
            verdict = "?"

        # Format values
        k_str = f"{k_val} ns" if k_val > 0 else "-"
        l_str = f"{l_val} ns" if l_val > 0 else "-"
        n_str = f"{n_val} ns" if n_val > 0 else "-"

        print(f"{test:<20} {k_str:>12} {l_str:>12} {n_str:>12} {ratio_str:>10} {verdict:>15}")

    print()
    print("Goal: Kevlar TCG within 10% of Linux TCG (ratio <= 1.10)")
    print()


def main():
    root_dir = Path(__file__).parent.parent
    import os
    os.chdir(root_dir)

    print("="*70)
    print("KEVLAR BENCHMARK SUITE")
    print("="*70)
    print()
    print("This will run benchmarks on:")
    print("  1. Kevlar on QEMU TCG")
    print("  2. Linux on QEMU TCG")
    print("  3. Native Linux (WSL)")
    print()

    # Run all benchmarks
    kevlar_results = run_kevlar_bench()
    linux_qemu_results = run_linux_qemu_bench()
    native_results = run_native_linux_bench()

    # Print comparison
    print_comparison_table(kevlar_results, linux_qemu_results, native_results)

    return 0


if __name__ == "__main__":
    sys.exit(main())
