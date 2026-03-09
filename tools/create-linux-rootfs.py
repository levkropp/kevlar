#!/usr/bin/env python3
"""
Create a minimal Linux root filesystem for QEMU benchmarking.

This creates an ext2 filesystem image with just the benchmark binary as /sbin/init.
The kernel will automatically run /sbin/init on boot.
"""

import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def create_rootfs_image(benchmark_binary, output_image, size_mb=50):
    """Create a minimal ext2 root filesystem with the benchmark as /sbin/init."""

    benchmark_path = Path(benchmark_binary)
    output_path = Path(output_image)

    if not benchmark_path.exists():
        print(f"Error: Benchmark binary not found: {benchmark_path}")
        return False

    print(f"Creating {size_mb}MB root filesystem image...")

    # Create empty image file
    with open(output_path, 'wb') as f:
        f.write(b'\0' * (size_mb * 1024 * 1024))

    # Format as ext2 using mke2fs (available in WSL)
    if platform.system() == "Windows":
        # Use WSL for filesystem operations
        wsl_output_path = str(output_path).replace('\\', '/').replace('C:/', '/mnt/c/')
        wsl_benchmark_path = str(benchmark_path).replace('\\', '/').replace('C:/', '/mnt/c/')

        print("Formatting filesystem (using WSL)...")
        result = subprocess.run(
            ["wsl", "mkfs.ext2", "-F", wsl_output_path],
            capture_output=True
        )
        if result.returncode != 0:
            print(f"Error formatting filesystem: {result.stderr.decode()}")
            return False

        # Mount and populate
        print("Populating filesystem...")
        script = f"""
set -e
MOUNT_DIR=$(mktemp -d)
sudo mount -o loop {wsl_output_path} $MOUNT_DIR
sudo mkdir -p $MOUNT_DIR/{{bin,sbin,etc,proc,sys,dev,tmp}}
sudo cp {wsl_benchmark_path} $MOUNT_DIR/sbin/init
sudo chmod +x $MOUNT_DIR/sbin/init
sudo umount $MOUNT_DIR
rmdir $MOUNT_DIR
"""
        result = subprocess.run(
            ["wsl", "bash", "-c", script],
            capture_output=True
        )
        if result.returncode != 0:
            print(f"Error populating filesystem: {result.stderr.decode()}")
            return False

    else:
        # Native Linux
        subprocess.run(["mkfs.ext2", "-F", str(output_path)], check=True)

        mount_dir = tempfile.mkdtemp()
        try:
            subprocess.run(["sudo", "mount", "-o", "loop", str(output_path), mount_dir], check=True)

            # Create directories
            for d in ["bin", "sbin", "etc", "proc", "sys", "dev", "tmp"]:
                os.makedirs(os.path.join(mount_dir, d), exist_ok=True)

            # Copy benchmark as /sbin/init
            init_path = os.path.join(mount_dir, "sbin", "init")
            subprocess.run(["sudo", "cp", str(benchmark_path), init_path], check=True)
            subprocess.run(["sudo", "chmod", "+x", init_path], check=True)

        finally:
            subprocess.run(["sudo", "umount", mount_dir], check=False)
            os.rmdir(mount_dir)

    print(f"✓ Root filesystem created: {output_path}")
    print(f"  Size: {output_path.stat().st_size // (1024*1024)} MB")
    print(f"  Benchmark: /sbin/init (runs automatically on boot)")

    return True


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <benchmark-binary> <output.img>")
        return 1

    benchmark = sys.argv[1]
    output = sys.argv[2]

    if not create_rootfs_image(benchmark, output):
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
