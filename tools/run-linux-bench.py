#!/usr/bin/env python3
"""
Run benchmark on Linux in QEMU for comparison with Kevlar.

Downloads a minimal Linux kernel and creates a tiny initramfs with just the benchmark.
Runs in QEMU (TCG on Windows, same as Kevlar) for fair comparison.

Usage:
    python tools/run-linux-bench.py
"""

import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path


def find_qemu():
    """Find QEMU executable."""
    # Try environment variable first
    if "QEMU_PATH" in os.environ and os.path.exists(os.environ["QEMU_PATH"]):
        return os.environ["QEMU_PATH"]

    # Try which/where
    tool = shutil.which("qemu-system-x86_64")
    if tool:
        return tool

    # Windows-specific paths
    if platform.system() == "Windows":
        common_paths = [
            r"C:\Program Files\qemu\qemu-system-x86_64.exe",
            r"C:\qemu\qemu-system-x86_64.exe",
        ]
        for path in common_paths:
            if os.path.exists(path):
                return path

    return None


def create_initramfs_with_benchmark(bench_binary, output_path):
    """Create minimal initramfs with just the benchmark binary."""
    import gzip
    import io

    print(f"Creating initramfs with benchmark...", file=sys.stderr)

    # Create CPIO archive (newc format)
    # Simple init script that runs the benchmark
    init_script = b"""#!/bin/sh
/bench
poweroff -f
"""

    # Build CPIO archive manually
    cpio_data = io.BytesIO()

    def add_file(name, content, mode=0o755):
        """Add a file to CPIO archive."""
        name_bytes = name.encode('ascii')
        content_bytes = content if isinstance(content, bytes) else content.encode('ascii')

        # CPIO newc header format
        # magic, ino, mode, uid, gid, nlink, mtime, filesize, devmajor, devminor,
        # rdevmajor, rdevminor, namesize, check
        header = "070701"  # magic
        header += f"{1:08x}"  # ino
        header += f"{mode:08x}"  # mode
        header += f"{0:08x}"  # uid
        header += f"{0:08x}"  # gid
        header += f"{1:08x}"  # nlink
        header += f"{0:08x}"  # mtime
        header += f"{len(content_bytes):08x}"  # filesize
        header += f"{0:08x}"  # devmajor
        header += f"{0:08x}"  # devminor
        header += f"{0:08x}"  # rdevmajor
        header += f"{0:08x}"  # rdevminor
        header += f"{len(name_bytes) + 1:08x}"  # namesize (includes null)
        header += f"{0:08x}"  # check

        cpio_data.write(header.encode('ascii'))
        cpio_data.write(name_bytes + b'\0')

        # Align to 4 bytes
        while cpio_data.tell() % 4 != 0:
            cpio_data.write(b'\0')

        cpio_data.write(content_bytes)

        # Align to 4 bytes
        while cpio_data.tell() % 4 != 0:
            cpio_data.write(b'\0')

    # Add init script
    add_file("init", init_script, 0o755)

    # Add benchmark binary
    with open(bench_binary, 'rb') as f:
        bench_content = f.read()
    add_file("bench", bench_content, 0o755)

    # Add TRAILER!!!
    add_file("TRAILER!!!", b"", 0)

    # Compress with gzip
    with gzip.open(output_path, 'wb') as f:
        f.write(cpio_data.getvalue())

    print(f"Initramfs created: {output_path}", file=sys.stderr)


def download_linux_kernel(output_path):
    """Download a minimal Linux kernel."""
    # Use a prebuilt TinyCore Linux kernel (very small)
    # Alternative: build from source or use distro kernel

    # For simplicity, we'll use buildroot's prebuilt kernel
    # URL for a minimal x86_64 kernel
    kernel_url = "https://github.com/buildroot/buildroot/releases/download/2023.02/buildroot-2023.02.tar.gz"

    print("Note: Using TinyCore Linux kernel for testing", file=sys.stderr)
    print("You can also use any Linux kernel image", file=sys.stderr)

    # Check if user has a kernel
    if os.path.exists("/boot/vmlinuz"):
        print("Found kernel at /boot/vmlinuz", file=sys.stderr)
        return "/boot/vmlinuz"

    # For now, ask user to provide kernel or use WSL
    print("\nTo run Linux benchmark, you need a Linux kernel image.", file=sys.stderr)
    print("Options:", file=sys.stderr)
    print("1. Extract from WSL: wsl cat /boot/vmlinuz > linux-kernel", file=sys.stderr)
    print("2. Download TinyCore: http://tinycorelinux.net/", file=sys.stderr)
    print("3. Use Docker: docker run --rm alpine cat /vmlinuz > linux-kernel", file=sys.stderr)
    return None


def run_linux_benchmark(kernel_path, initramfs_path, timeout=60):
    """Run Linux with benchmark in QEMU."""
    qemu = find_qemu()
    if not qemu:
        print("Error: qemu-system-x86_64 not found", file=sys.stderr)
        return None

    print(f"Running Linux benchmark in QEMU (timeout: {timeout}s)...", file=sys.stderr)
    print(f"QEMU: {qemu}", file=sys.stderr)
    print(f"Kernel: {kernel_path}", file=sys.stderr)

    # QEMU command
    cmd = [
        qemu,
        "-kernel", kernel_path,
        "-initrd", initramfs_path,
        "-append", "console=ttyS0 quiet",
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
        print("Timeout - collecting partial results", file=sys.stderr)

    return output


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


def main():
    root_dir = Path(__file__).parent.parent

    # Check for benchmark binary
    bench_linux = root_dir / "benchmarks" / "bench.linux"
    if not bench_linux.exists():
        print(f"Error: {bench_linux} not found", file=sys.stderr)
        print("Compile it with: wsl bash -c 'cd benchmarks && gcc -static -O2 -o bench.linux bench.c'", file=sys.stderr)
        return 1

    # Create initramfs
    with tempfile.NamedTemporaryFile(suffix=".cpio.gz", delete=False) as f:
        initramfs_path = f.name

    try:
        create_initramfs_with_benchmark(bench_linux, initramfs_path)

        # Get kernel - try WSL first
        kernel_path = None

        # Try extracting from WSL
        if shutil.which("wsl"):
            print("Extracting kernel from WSL...", file=sys.stderr)
            wsl_kernel = root_dir / "build" / "linux-kernel"
            wsl_kernel.parent.mkdir(exist_ok=True)

            result = subprocess.run(
                ["wsl", "bash", "-c", "cat /boot/vmlinuz-$(uname -r)"],
                capture_output=True,
                check=False
            )

            if result.returncode == 0 and len(result.stdout) > 1000000:
                with open(wsl_kernel, 'wb') as f:
                    f.write(result.stdout)
                kernel_path = str(wsl_kernel)
                print(f"Extracted kernel: {kernel_path} ({len(result.stdout)} bytes)", file=sys.stderr)
            else:
                # Try generic kernel path
                result = subprocess.run(
                    ["wsl", "bash", "-c", "ls /boot/vmlinuz* | head -1"],
                    capture_output=True,
                    text=True,
                    check=False
                )
                if result.returncode == 0:
                    wsl_kernel_name = result.stdout.strip()
                    result2 = subprocess.run(
                        ["wsl", "bash", "-c", f"cat {wsl_kernel_name}"],
                        capture_output=True,
                        check=False
                    )
                    if result2.returncode == 0:
                        with open(wsl_kernel, 'wb') as f:
                            f.write(result2.stdout)
                        kernel_path = str(wsl_kernel)

        if not kernel_path:
            print("\nCouldn't automatically extract Linux kernel.", file=sys.stderr)
            print("Please extract manually:", file=sys.stderr)
            print("  wsl bash -c 'cat /boot/vmlinuz-$(uname -r)' > build/linux-kernel", file=sys.stderr)
            return 1

        # Run benchmark
        output = run_linux_benchmark(kernel_path, initramfs_path)
        if not output:
            return 1

        # Save output
        output_file = root_dir / "linux-benchmark-output.txt"
        with open(output_file, 'w') as f:
            f.write(output)
        print(f"Full output saved to: {output_file}", file=sys.stderr)

        # Parse results
        results = parse_benchmark_results(output)

        if not results:
            print("\nNo benchmark results found!", file=sys.stderr)
            print("Check linux-benchmark-output.txt for details", file=sys.stderr)
            return 1

        # Print results
        print("\n=== Linux Benchmark Results (QEMU TCG) ===\n")
        print(f"{'Test':<20} {'Iterations':>12} {'Per-iter (ns)':>15}")
        print("-" * 55)

        for test_name in sorted(results.keys()):
            data = results[test_name]
            print(f"{test_name:<20} {data['iterations']:>12,} {data['per_iter_ns']:>15,}")

        print()
        return 0

    finally:
        # Cleanup
        if os.path.exists(initramfs_path):
            os.unlink(initramfs_path)


if __name__ == "__main__":
    sys.exit(main())
