#!/usr/bin/env python3
"""Convert an Alpine ext2 disk image into a cpio.gz initramfs.

Used by the linux-on-hvf harness to boot Alpine's prebuilt arm64
kernel against the same userspace Kevlar uses, so per-program
tests have a one-command Linux baseline.

Usage:
    tools/build-alpine-cpio.py SRC.img DST.cpio.gz

Caches: skips the work if DST is newer than SRC.

Pipeline:
    1. debugfs `rdump /` SRC into a temp dir.
    2. find . -print | cpio -o -H newc | gzip → DST.

Note: requires e2fsprogs (`brew install e2fsprogs` on macOS) for
the debugfs binary, and standard cpio + gzip from the host.
"""
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def find_debugfs():
    """Locate debugfs — prefer brew's e2fsprogs since macOS's
    minimal debugfs in /System/... lacks rdump."""
    candidates = [
        "/opt/homebrew/opt/e2fsprogs/sbin/debugfs",
        "/usr/local/opt/e2fsprogs/sbin/debugfs",
        shutil.which("debugfs"),
    ]
    for c in candidates:
        if c and os.path.isfile(c):
            return c
    sys.exit("error: debugfs not found; install e2fsprogs "
             "(`brew install e2fsprogs`)")


def needs_rebuild(src: Path, dst: Path) -> bool:
    if not dst.exists():
        return True
    return src.stat().st_mtime > dst.stat().st_mtime


def main():
    if len(sys.argv) != 3:
        sys.exit(f"usage: {sys.argv[0]} SRC.img DST.cpio.gz")

    src = Path(sys.argv[1]).resolve()
    dst = Path(sys.argv[2]).resolve()

    if not src.exists():
        sys.exit(f"error: source image {src} does not exist")

    if not needs_rebuild(src, dst):
        # Cached — print a one-line note so the Makefile log shows why
        # we skipped, then exit clean.
        print(f"  CACHED  {dst} (newer than {src})", file=sys.stderr)
        return

    debugfs = find_debugfs()
    dst.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="alpine-cpio-") as td:
        tdpath = Path(td)
        print(f"  EXTRACT {src} → {tdpath} (via debugfs rdump)",
              file=sys.stderr)
        # `rdump / <dest>` recursively dumps all files.  Some chown
        # ops will fail on macOS (we don't have root + can't preserve
        # uids/gids), but the file contents land correctly — that's
        # what matters for the cpio.
        proc = subprocess.run(
            [debugfs, "-R", f"rdump / {tdpath}", str(src)],
            capture_output=True, text=True,
        )
        if proc.returncode != 0:
            print(proc.stderr, file=sys.stderr)
            sys.exit("error: debugfs rdump failed")

        # Pack as newc cpio + gzip.  newc is the kernel's default
        # initramfs format and is well-supported by Linux's
        # initramfs unpacker.
        print(f"  CPIO    → {dst}", file=sys.stderr)
        with open(dst, "wb") as out_f:
            find = subprocess.Popen(
                ["find", ".", "-print"],
                cwd=str(tdpath),
                stdout=subprocess.PIPE,
            )
            cpio = subprocess.Popen(
                ["cpio", "-o", "-H", "newc"],
                cwd=str(tdpath),
                stdin=find.stdout,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
            find.stdout.close()  # let cpio see EOF when find exits
            gz = subprocess.Popen(
                ["gzip", "-c"],
                stdin=cpio.stdout,
                stdout=out_f,
            )
            cpio.stdout.close()
            find.wait()
            cpio.wait()
            gz.wait()

    size_mb = dst.stat().st_size // (1024 * 1024)
    print(f"  DONE    {dst} ({size_mb} MB)", file=sys.stderr)


if __name__ == "__main__":
    main()
