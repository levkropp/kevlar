#!/usr/bin/env python3
"""Build Alpine ext2 disk image natively (no Docker required).

Downloads Alpine minirootfs, sets up apk config, and creates a 64MB
ext2 disk image using mke2fs.

Usage: python3 tools/build-alpine-disk.py build/alpine-disk.img
"""
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "build" / "native-cache" / "src"

ALPINE_ROOTFS_URL = (
    "https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/x86_64/"
    "alpine-minirootfs-3.21.3-x86_64.tar.gz"
)


def download(url, dest):
    dest = Path(dest)
    if dest.exists():
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    print(f"  DL  {Path(url).name}", file=sys.stderr)
    urllib.request.urlretrieve(url, dest)
    return dest


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <output.img>", file=sys.stderr)
        sys.exit(1)

    output = sys.argv[1]
    tarball = download(ALPINE_ROOTFS_URL,
                       CACHE / "alpine-minirootfs-3.21.3-x86_64.tar.gz")

    with tempfile.TemporaryDirectory() as tmpdir:
        alpine_root = Path(tmpdir) / "alpine-root"
        alpine_root.mkdir()

        # Extract rootfs
        print("  EXTRACT  Alpine minirootfs", file=sys.stderr)
        with tarfile.open(tarball, "r:gz") as tar:
            try:
                tar.extractall(path=alpine_root, filter="fully_trusted")
            except TypeError:
                tar.extractall(path=alpine_root)

        # Configure apk
        repos = alpine_root / "etc" / "apk" / "repositories"
        repos.parent.mkdir(parents=True, exist_ok=True)
        repos.write_text("http://dl-cdn.alpinelinux.org/alpine/v3.21/main\n")

        resolv = alpine_root / "etc" / "resolv.conf"
        resolv.write_text("nameserver 10.0.2.3\n")

        (alpine_root / "var" / "cache" / "apk").mkdir(parents=True, exist_ok=True)
        (alpine_root / "tmp").mkdir(exist_ok=True)

        # Create 64MB ext2 disk image
        print("  MKDISK  64MB ext2", file=sys.stderr)
        subprocess.run(
            ["dd", "if=/dev/zero", f"of={output}",
             "bs=1M", "count=64"],
            check=True, capture_output=True)
        subprocess.run(
            ["mke2fs", "-t", "ext2", "-d", str(alpine_root), output],
            check=True, capture_output=True)

    print(f"  DONE  {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
