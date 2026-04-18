#!/usr/bin/env python3
"""extract-alpine-rootfs — dump an ext2/ext4 Kevlar disk image to a directory
so the host can run strace against the same Alpine binaries.

Uses `debugfs -R 'rdump / DIR' IMAGE` which works without sudo (no mount).

Usage:
    tools/extract-alpine-rootfs.py build/alpine-xfce.img build/alpine-xfce-rootfs
"""
import argparse
import shutil
import subprocess
import sys
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("image", help="ext2/ext4 disk image")
    ap.add_argument("dest", help="destination directory (created if missing)")
    ap.add_argument("--force", action="store_true",
                    help="delete dest first")
    args = ap.parse_args()

    img = Path(args.image)
    dest = Path(args.dest)
    if not img.exists():
        print(f"[extract] image not found: {img}", file=sys.stderr)
        return 2

    if dest.exists():
        if args.force:
            shutil.rmtree(dest)
        else:
            print(f"[extract] {dest} exists; pass --force to overwrite",
                  file=sys.stderr)
            return 2
    dest.mkdir(parents=True, exist_ok=True)

    print(f"[extract] debugfs rdump / -> {dest} (this takes ~15s for 512MB)",
          file=sys.stderr)
    r = subprocess.run(
        ["debugfs", "-R", f"rdump / {dest}", str(img)],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        print(r.stderr, file=sys.stderr)
        return r.returncode

    # Normalize permissions so the user can read/write everything.
    # debugfs preserves image perms (incl. 0o000 files from root ownership).
    subprocess.run(["chmod", "-R", "u+rwX", str(dest)], check=True)

    print(f"[extract] done. {sum(1 for _ in dest.rglob('*'))} entries extracted.",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
