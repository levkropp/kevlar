#!/usr/bin/env python3
"""
Compare Kevlar vs Linux benchmark results.

Runs benchmarks on:
1. Kevlar in QEMU (TCG on Windows, or KVM on Linux)
2. Linux native (WSL on Windows, or bare metal on Linux)

Usage:
    python tools/compare-bench.py
    python tools/compare-bench.py --kevlar-only  # Skip Linux
    python tools/compare-bench.py --linux-only   # Skip Kevlar
"""

import argparse
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path


def run_kevlar_benchmark(timeout=60):
    """Run Kevlar benchmark."""
    root_dir = Path(__file__).parent.parent

    print("=" * 70)
    print("Running Kevlar benchmark in QEMU...")
    print("=" * 70)

    run_bench_script = root_dir / "tools" / "run-bench.py"
    result = subprocess.run(
        [sys.executable, str(run_bench_script), "--timeout", str(timeout), "--no-build"],
        capture_output=True,
        text=True,
        cwd=root_dir
    )

    output = result.stdout + result.stderr

    # Debug: show what we got
    print(f"Kevlar benchmark output ({len(output)} chars)", file=sys.stderr)

    # Parse results
    results = parse_benchmark_results(output)

    if not results:
        print("Warning: No results parsed from Kevlar benchmark", file=sys.stderr)
        # Try to parse from the table format too
        table_pattern = re.compile(r'(\S+)\s+(\d{1,3}(?:,\d{3})*)\s+(\d{1,3}(?:,\d{3})*)')
        for line in output.splitlines():
            match = table_pattern.search(line)
            if match:
                test_name, iterations_str, per_iter_str = match.groups()
                if test_name in ["getpid", "mmap_fault", "open_close", "pipe", "read_null", "stat", "write_null"]:
                    iterations = int(iterations_str.replace(',', ''))
                    per_iter_ns = int(per_iter_str.replace(',', ''))
                    results[test_name] = {
                        "iterations": iterations,
                        "total_ns": iterations * per_iter_ns,
                        "per_iter_ns": per_iter_ns
                    }

    return results


def create_linux_initramfs(bench_binary, output_path):
    """Create a minimal initramfs with the benchmark."""
    import gzip
    import io
    import struct

    print(f"Creating Linux initramfs...", file=sys.stderr)

    # Read benchmark binary
    with open(bench_binary, 'rb') as f:
        bench_content = f.read()

    # Create init script
    init_script = b"""#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
/bench
sync
poweroff -f
"""

    # Build CPIO archive (newc format)
    cpio_data = io.BytesIO()

    def write_cpio_entry(name, content, mode=0o100755, is_dir=False):
        """Write a CPIO newc entry."""
        if is_dir:
            mode = 0o040755
            content = b""

        name_bytes = name.encode('ascii') if isinstance(name, str) else name
        content_bytes = content if isinstance(content, bytes) else content.encode('ascii')

        # CPIO newc header (110 bytes)
        ino = 1
        nlink = 2 if is_dir else 1

        header = b"070701"  # magic
        header += f"{ino:08x}".encode('ascii')  # ino
        header += f"{mode:08x}".encode('ascii')  # mode
        header += f"{0:08x}".encode('ascii')  # uid
        header += f"{0:08x}".encode('ascii')  # gid
        header += f"{nlink:08x}".encode('ascii')  # nlink
        header += f"{0:08x}".encode('ascii')  # mtime
        header += f"{len(content_bytes):08x}".encode('ascii')  # filesize
        header += f"{0:08x}".encode('ascii')  # devmajor
        header += f"{0:08x}".encode('ascii')  # devminor
        header += f"{0:08x}".encode('ascii')  # rdevmajor
        header += f"{0:08x}".encode('ascii')  # rdevminor
        header += f"{len(name_bytes) + 1:08x}".encode('ascii')  # namesize
        header += f"{0:08x}".encode('ascii')  # check

        cpio_data.write(header)
        cpio_data.write(name_bytes + b'\x00')

        # Align to 4 bytes
        while cpio_data.tell() % 4 != 0:
            cpio_data.write(b'\x00')

        if content_bytes:
            cpio_data.write(content_bytes)
            # Align to 4 bytes
            while cpio_data.tell() % 4 != 0:
                cpio_data.write(b'\x00')

    # Add directories
    write_cpio_entry(".", b"", is_dir=True)
    write_cpio_entry("proc", b"", is_dir=True)
    write_cpio_entry("sys", b"", is_dir=True)

    # Add files
    write_cpio_entry("init", init_script, 0o100755)
    write_cpio_entry("bench", bench_content, 0o100755)

    # Trailer
    write_cpio_entry("TRAILER!!!", b"", 0o100644)

    # Compress with gzip
    with gzip.open(output_path, 'wb', compresslevel=9) as f:
        f.write(cpio_data.getvalue())

    print(f"Initramfs created: {output_path} ({len(cpio_data.getvalue())} bytes, compressed)", file=sys.stderr)


def run_linux_benchmark_qemu(timeout=60):
    """Run Linux benchmark in QEMU TCG (same as Kevlar)."""
    root_dir = Path(__file__).parent.parent

    print("\n" + "=" * 70)
    print("Running Linux benchmark in QEMU TCG...")
    print("=" * 70)

    bench_linux = root_dir / "benchmarks" / "bench.linux"
    if not bench_linux.exists():
        print(f"Error: {bench_linux} not found", file=sys.stderr)
        return None

    # Create initramfs
    build_dir = root_dir / "build"
    build_dir.mkdir(exist_ok=True)
    initramfs_path = build_dir / "linux-bench.initramfs.gz"

    create_linux_initramfs(bench_linux, initramfs_path)

    # Get Linux kernel - try multiple sources
    kernel_path = build_dir / "vmlinuz64"
    if not kernel_path.exists():
        kernel_path = build_dir / "linux-kernel"

    if not kernel_path.exists():
        print("Downloading Linux kernel...", file=sys.stderr)

        # Download TinyCore Linux kernel (small, fast download)
        kernel_url = "http://tinycorelinux.net/15.x/x86_64/release/distribution_files/vmlinuz64"
        kernel_path = build_dir / "vmlinuz64"

        try:
            import urllib.request
            with urllib.request.urlopen(kernel_url, timeout=30) as response:
                kernel_data = response.read()

            with open(kernel_path, 'wb') as f:
                f.write(kernel_data)

            print(f"Kernel downloaded: {len(kernel_data)} bytes", file=sys.stderr)

        except Exception as e:
            print(f"Error downloading kernel: {e}", file=sys.stderr)
            print("Please download manually:", file=sys.stderr)
            print(f"  curl -o {kernel_path} {kernel_url}", file=sys.stderr)
            return None

    # Find QEMU
    qemu = shutil.which("qemu-system-x86_64")
    if not qemu and platform.system() == "Windows":
        qemu = r"C:\Program Files\qemu\qemu-system-x86_64.exe"

    if not qemu or not Path(qemu).exists():
        print("Error: QEMU not found", file=sys.stderr)
        return None

    print(f"QEMU: {qemu}", file=sys.stderr)
    print(f"Kernel: {kernel_path}", file=sys.stderr)

    # Run QEMU
    cmd = [
        qemu,
        "-kernel", str(kernel_path),
        "-initrd", str(initramfs_path),
        "-append", "console=ttyS0 quiet panic=-1",
        "-m", "1024",
        "-cpu", "Icelake-Server",
        "-nographic",
        "-no-reboot",
    ]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout
        )
        output = result.stdout + result.stderr
    except subprocess.TimeoutExpired as e:
        output = (e.stdout or "") + (e.stderr or "")
        print("Timeout - collecting results", file=sys.stderr)

    # Save output for debugging
    with open(root_dir / "linux-qemu-output.txt", 'w') as f:
        f.write(output)

    # Parse results
    results = parse_benchmark_results(output)
    return results


def run_linux_benchmark_wsl():
    """Run Linux benchmark natively in WSL."""
    root_dir = Path(__file__).parent.parent

    print("\n" + "=" * 70)
    print("Running Linux benchmark in WSL (native)...")
    print("=" * 70)

    bench_linux = root_dir / "benchmarks" / "bench.linux"

    if not bench_linux.exists():
        print(f"Error: {bench_linux} not found", file=sys.stderr)
        print("Compile it with: wsl bash -c 'cd benchmarks && gcc -static -O2 -o bench.linux bench.c'", file=sys.stderr)
        return None

    # Convert path to WSL format
    wsl_path = f"/mnt/c/Users/{bench_linux.parts[2]}/kevlar/benchmarks/bench.linux"

    result = subprocess.run(
        ["wsl", "bash", "-c", wsl_path],
        capture_output=True,
        text=True
    )

    if result.returncode != 0:
        print(f"Error running Linux benchmark: {result.stderr}", file=sys.stderr)
        return None

    output = result.stdout + result.stderr

    # Parse results
    results = parse_benchmark_results(output)
    return results


def parse_benchmark_results(output):
    """Parse BENCH lines from output."""
    results = {}
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


def print_comparison(kevlar_results, linux_qemu_results, linux_wsl_results):
    """Print side-by-side comparison."""
    print("\n" + "=" * 110)
    print("BENCHMARK COMPARISON: Kevlar vs Linux (TCG) vs Linux (native)")
    print("=" * 110)

    if not kevlar_results and not linux_qemu_results and not linux_wsl_results:
        print("No results to compare!")
        return

    # Get all test names
    all_tests = set()
    if kevlar_results:
        all_tests.update(kevlar_results.keys())
    if linux_qemu_results:
        all_tests.update(linux_qemu_results.keys())
    if linux_wsl_results:
        all_tests.update(linux_wsl_results.keys())

    print(f"\n{'Test':<15} {'Kevlar TCG':>13} {'Linux TCG':>13} {'Linux WSL':>13} {'vs Linux TCG':>15} {'vs Linux WSL':>15}")
    print("-" * 110)

    for test_name in sorted(all_tests):
        kevlar_ns = kevlar_results.get(test_name, {}).get("per_iter_ns", 0) if kevlar_results else 0
        linux_qemu_ns = linux_qemu_results.get(test_name, {}).get("per_iter_ns", 0) if linux_qemu_results else 0
        linux_wsl_ns = linux_wsl_results.get(test_name, {}).get("per_iter_ns", 0) if linux_wsl_results else 0

        # Calculate ratios
        if kevlar_ns and linux_qemu_ns:
            ratio_qemu = kevlar_ns / linux_qemu_ns
            if ratio_qemu < 1.1:
                verdict_qemu = f"{ratio_qemu:.2f}x (good!)"
            else:
                verdict_qemu = f"{ratio_qemu:.2f}x"
        else:
            ratio_qemu = 0
            verdict_qemu = "N/A"

        if kevlar_ns and linux_wsl_ns:
            ratio_wsl = kevlar_ns / linux_wsl_ns
            verdict_wsl = f"{ratio_wsl:.2f}x"
        else:
            ratio_wsl = 0
            verdict_wsl = "N/A"

        kevlar_display = f"{kevlar_ns:,}" if kevlar_ns else "N/A"
        linux_qemu_display = f"{linux_qemu_ns:,}" if linux_qemu_ns else "N/A"
        linux_wsl_display = f"{linux_wsl_ns:,}" if linux_wsl_ns else "N/A"

        print(f"{test_name:<15} {kevlar_display:>13} {linux_qemu_display:>13} {linux_wsl_display:>13} {verdict_qemu:>15} {verdict_wsl:>15}")

    print()

    # Highlight mmap_fault specifically
    if "mmap_fault" in (kevlar_results or {}) or "mmap_fault" in (linux_qemu_results or {}) or "mmap_fault" in (linux_wsl_results or {}):
        print("=" * 110)
        print(f"MMAP_FAULT (demand paging) SUMMARY:")

        if kevlar_results and "mmap_fault" in kevlar_results:
            kevlar_mmap = kevlar_results["mmap_fault"]["per_iter_ns"]
            print(f"  Kevlar (QEMU TCG):      {kevlar_mmap:,} ns/fault")

        if linux_qemu_results and "mmap_fault" in linux_qemu_results:
            linux_qemu_mmap = linux_qemu_results["mmap_fault"]["per_iter_ns"]
            print(f"  Linux (QEMU TCG):       {linux_qemu_mmap:,} ns/fault")

            if kevlar_results and "mmap_fault" in kevlar_results:
                ratio_qemu = kevlar_mmap / linux_qemu_mmap
                print(f"  Kevlar vs Linux TCG:    {ratio_qemu:.2f}x")
                if ratio_qemu < 1.1:
                    print(f"  \033[92m✓ Kevlar matches Linux on same platform (TCG)!\033[0m")
                elif ratio_qemu < 1.5:
                    print(f"  \033[93m△ Kevlar is {(ratio_qemu-1)*100:.1f}% slower than Linux (TCG)\033[0m")
                else:
                    print(f"  \033[91m✗ Kevlar is {(ratio_qemu-1)*100:.1f}% slower than Linux (TCG)\033[0m")

        if linux_wsl_results and "mmap_fault" in linux_wsl_results:
            linux_wsl_mmap = linux_wsl_results["mmap_fault"]["per_iter_ns"]
            print(f"  Linux (WSL2 native):    {linux_wsl_mmap:,} ns/fault (reference)")

        print("=" * 110)

    # Environment info
    print("\nEnvironment:")
    print(f"  Platform:     {platform.system()} {platform.release()}")
    print(f"  Kevlar:       QEMU TCG (software emulation)")
    print(f"  Linux QEMU:   QEMU TCG (software emulation, same as Kevlar)")
    print(f"  Linux WSL:    WSL2 native (reference baseline)")
    print()


def main():
    parser = argparse.ArgumentParser(description="Compare Kevlar vs Linux benchmarks")
    parser.add_argument("--kevlar-only", action="store_true",
                       help="Only run Kevlar benchmark")
    parser.add_argument("--linux-only", action="store_true",
                       help="Only run Linux benchmarks")
    parser.add_argument("--skip-wsl", action="store_true",
                       help="Skip Linux WSL native benchmark")
    parser.add_argument("--skip-linux-qemu", action="store_true",
                       help="Skip Linux QEMU benchmark")
    parser.add_argument("--timeout", type=int, default=60,
                       help="QEMU timeout in seconds (default: 60)")

    args = parser.parse_args()

    kevlar_results = None
    linux_qemu_results = None
    linux_wsl_results = None

    if not args.linux_only:
        kevlar_results = run_kevlar_benchmark(args.timeout)

    if not args.kevlar_only:
        if shutil.which("wsl"):
            # Run Linux in QEMU TCG (apples-to-apples with Kevlar)
            if not args.skip_linux_qemu:
                linux_qemu_results = run_linux_benchmark_qemu(args.timeout)

            # Run Linux native in WSL (reference)
            if not args.skip_wsl:
                linux_wsl_results = run_linux_benchmark_wsl()
        else:
            print("\nWSL not available - skipping Linux benchmarks", file=sys.stderr)
            print("Install WSL to compare with Linux", file=sys.stderr)

    print_comparison(kevlar_results, linux_qemu_results, linux_wsl_results)

    return 0


if __name__ == "__main__":
    sys.exit(main())
