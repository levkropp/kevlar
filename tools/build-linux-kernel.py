#!/usr/bin/env python3
"""
Build a minimal Linux kernel for benchmarking in QEMU.

This creates a kernel with just enough features to:
- Boot in QEMU
- Use initramfs as root
- Run our benchmark binary
- Output to serial console
"""

import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path


def download_kernel_source(version="6.6.8", dest_dir=None):
    """Download and extract Linux kernel source."""
    if dest_dir is None:
        dest_dir = Path.cwd() / "build"

    dest_dir = Path(dest_dir)
    dest_dir.mkdir(exist_ok=True)

    kernel_dir = dest_dir / f"linux-{version}"
    if kernel_dir.exists():
        print(f"Kernel source already exists: {kernel_dir}")
        return kernel_dir

    url = f"https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-{version}.tar.xz"
    tarball = dest_dir / f"linux-{version}.tar.xz"

    if not tarball.exists():
        print(f"Downloading Linux {version} from kernel.org...")
        print(f"URL: {url}")

        with urllib.request.urlopen(url) as response:
            total_size = int(response.headers.get('Content-Length', 0))
            downloaded = 0

            with open(tarball, 'wb') as f:
                while True:
                    chunk = response.read(1024 * 1024)  # 1MB chunks
                    if not chunk:
                        break
                    f.write(chunk)
                    downloaded += len(chunk)
                    if total_size:
                        percent = (downloaded / total_size) * 100
                        print(f"\rDownloading: {percent:.1f}% ({downloaded // (1024*1024)}MB / {total_size // (1024*1024)}MB)", end='')
        print()

    print(f"Extracting kernel source...")
    with tarfile.open(tarball, 'r:xz') as tar:
        tar.extractall(dest_dir)

    return kernel_dir


def create_minimal_config(kernel_dir):
    """Create minimal kernel config for QEMU benchmarking."""
    config_content = """
# Minimal config for QEMU benchmarking
CONFIG_64BIT=y
CONFIG_X86_64=y
CONFIG_SMP=y
CONFIG_LOCALVERSION="-bench"

# Essential
CONFIG_PRINTK=y
CONFIG_EARLY_PRINTK=y
CONFIG_SERIAL_8250=y
CONFIG_SERIAL_8250_CONSOLE=y

# CRITICAL: Binary format support for executing ELF binaries
CONFIG_BINFMT_ELF=y
CONFIG_BINFMT_SCRIPT=y
CONFIG_EXEC_STACK=y

# Initramfs support (critical!)
CONFIG_BLK_DEV_INITRD=y
CONFIG_INITRAMFS_SOURCE=""
CONFIG_RD_GZIP=y

# Basic filesystems
CONFIG_PROC_FS=y
CONFIG_SYSFS=y
CONFIG_TMPFS=y
CONFIG_DEVTMPFS=y
CONFIG_DEVTMPFS_MOUNT=y

# Disable unnecessary features
CONFIG_MODULES=n
CONFIG_NETWORK=n
CONFIG_WIRELESS=n
CONFIG_INET=n
CONFIG_BLK_DEV=n
CONFIG_ATA=n
CONFIG_SCSI=n
CONFIG_USB=n
CONFIG_SOUND=n
CONFIG_DRM=n
CONFIG_FB=n

# Basic devices for QEMU
CONFIG_SERIAL_8250_NR_UARTS=4
CONFIG_SERIAL_8250_RUNTIME_UARTS=4

# Disable security features for speed
CONFIG_SECURITY=n
CONFIG_SECURITYFS=n

# Optimize for size
CONFIG_CC_OPTIMIZE_FOR_SIZE=y
"""

    config_file = kernel_dir / ".config"

    # Use allnoconfig as base, then add our options
    print("Generating minimal kernel config...")

    # On Windows, we must use WSL's make because Git Bash make sees CURDIR as C:/...
    # The kernel Makefile checks $(CURDIR) and rejects paths with colons
    if platform.system() == "Windows":
        # Convert to WSL path (/mnt/c/...)
        wsl_path = str(kernel_dir).replace('\\', '/').replace('C:/', '/mnt/c/')
        subprocess.run(
            ["wsl", "-u", "root", "bash", "-c", f"cd '{wsl_path}' && make allnoconfig"],
            check=True,
            stdout=subprocess.DEVNULL
        )
    else:
        subprocess.run(
            ["make", "allnoconfig"],
            cwd=kernel_dir,
            check=True,
            stdout=subprocess.DEVNULL
        )

    # Append our config
    with open(config_file, 'a') as f:
        f.write(config_content)

    # Run olddefconfig to resolve dependencies
    if platform.system() == "Windows":
        wsl_path = str(kernel_dir).replace('\\', '/').replace('C:/', '/mnt/c/')
        subprocess.run(
            ["wsl", "-u", "root", "bash", "-c", f"cd '{wsl_path}' && make olddefconfig"],
            check=True,
            stdout=subprocess.DEVNULL
        )
    else:
        subprocess.run(
            ["make", "olddefconfig"],
            cwd=kernel_dir,
            check=True,
            stdout=subprocess.DEVNULL
        )

    print(f"Config created: {config_file}")


def build_kernel(kernel_dir, num_jobs=None):
    """Build the kernel."""
    if num_jobs is None:
        import multiprocessing
        num_jobs = multiprocessing.cpu_count()

    print(f"\nBuilding kernel with {num_jobs} parallel jobs...")

    # On Windows, use WSL's make (Git Bash make sees CURDIR with colons)
    if platform.system() == "Windows":
        wsl_path = str(kernel_dir).replace('\\', '/').replace('C:/', '/mnt/c/')
        subprocess.run(
            ["wsl", "-u", "root", "bash", "-c", f"cd '{wsl_path}' && make -j{num_jobs} bzImage"],
            check=True
        )
    else:
        subprocess.run(
            ["make", f"-j{num_jobs}", "bzImage"],
            cwd=kernel_dir,
            check=True
        )

    bzimage = kernel_dir / "arch" / "x86" / "boot" / "bzImage"
    if not bzimage.exists():
        raise RuntimeError("Kernel build succeeded but bzImage not found!")

    return bzimage


def has_path_issues(path_str):
    """Check if path has characters that break Linux kernel Makefile."""
    # Linux kernel Makefile doesn't allow spaces or colons (or periods, which it treats like colons)
    if ' ' in path_str or ':' in path_str:
        return True
    # Check for periods in path components (username like "26200.7462" breaks it)
    parts = Path(path_str).parts
    for part in parts:
        if '.' in part and part not in ['.', '..'] and not part.endswith('.tar.xz'):
            return True
    return False


def win_path_to_bash(path_str):
    """Convert Windows path to Git Bash Unix-style path."""
    # Convert C:/foo or C:\foo to /c/foo
    path_str = str(path_str).replace('\\', '/')
    if len(path_str) >= 2 and path_str[1] == ':':
        drive = path_str[0].lower()
        rest = path_str[2:] if len(path_str) > 2 else ""
        return f"/{drive}{rest}"
    return path_str


def main():
    root_dir = Path(__file__).parent.parent
    build_dir = root_dir / "build"

    print("=" * 70)
    print("Building minimal Linux kernel for benchmarking")
    print("=" * 70)

    # Check if we need to use a safe build location
    use_temp = False
    if platform.system() == "Windows":
        if has_path_issues(str(root_dir)):
            print(f"\nDetected problematic path characters in: {root_dir}")
            print("Linux kernel build requires path without spaces, colons, or periods")
            print("Using temporary build directory...\n")
            use_temp = True

    if use_temp:
        # Build in a safe temporary location (Git Bash path without problematic chars)
        # On Windows, Path("/linux") resolves to \linux, but we want C:/linux
        if platform.system() == "Windows":
            temp_build_dir = Path("C:/linux")
        else:
            temp_build_dir = Path("/linux")
        temp_build_dir.mkdir(parents=True, exist_ok=True)

        # Download/extract to temp location
        kernel_dir = download_kernel_source(dest_dir=temp_build_dir)

        # Create config and build
        create_minimal_config(kernel_dir)
        bzimage = build_kernel(kernel_dir)

        # Copy result back to original build dir
        build_dir.mkdir(exist_ok=True)
        output_kernel = build_dir / "vmlinuz-bench"
        shutil.copy2(bzimage, output_kernel)

        print(f"\nCleaning up temporary build directory...")
        # Keep the tarball for future use, delete the extracted source
        kernel_src = temp_build_dir / kernel_dir.name
        if kernel_src.exists():
            shutil.rmtree(kernel_src)
    else:
        # Build in place (Linux or Windows with safe path)
        kernel_dir = download_kernel_source(dest_dir=build_dir)
        create_minimal_config(kernel_dir)
        bzimage = build_kernel(kernel_dir)

        output_kernel = build_dir / "vmlinuz-bench"
        shutil.copy2(bzimage, output_kernel)

    print("\n" + "=" * 70)
    print(f"SUCCESS! Kernel built: {output_kernel}")
    print(f"Size: {output_kernel.stat().st_size // 1024} KB")
    print("=" * 70)
    print("\nYou can now use this kernel for Linux benchmarks in QEMU")

    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("\n\nBuild interrupted by user")
        sys.exit(1)
    except Exception as e:
        print(f"\n\nError: {e}", file=sys.stderr)
        sys.exit(1)
