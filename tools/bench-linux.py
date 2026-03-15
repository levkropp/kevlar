#!/usr/bin/env python3
"""
Run bench.c under Linux KVM as a baseline for bench-report.py.

Compiles bench.c statically, wraps it in a minimal initramfs, and
boots it under the host's own kernel with KVM.

Usage:
    python3 tools/bench-linux.py          # quick (256 iters)
    python3 tools/bench-linux.py --full   # full (4096 iters)
"""
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BENCH_SRC = ROOT / "benchmarks" / "bench.c"
BENCH_BIN = ROOT / "build" / "bench.linux"
QEMU = "qemu-system-x86_64"
TIMEOUT = 180


def find_linux_kernel():
    """Find the host's vmlinuz."""
    release = os.uname().release
    candidates = [
        Path(f"/lib/modules/{release}/vmlinuz"),
        Path(f"/boot/vmlinuz-{release}"),
        Path("/boot/vmlinuz"),
    ]
    for p in candidates:
        if p.exists():
            return p
    return None


def main():
    full = "--full" in sys.argv

    # Build bench.c
    os.makedirs(ROOT / "build", exist_ok=True)
    if not BENCH_BIN.exists() or BENCH_SRC.stat().st_mtime > BENCH_BIN.stat().st_mtime:
        print("Compiling bench.c for Linux...", flush=True)
        r = subprocess.run(
            ["gcc", "-static", "-O2", "-o", str(BENCH_BIN), str(BENCH_SRC)],
            capture_output=True, text=True,
        )
        if r.returncode != 0:
            print(f"gcc failed: {r.stderr}", file=sys.stderr)
            return 1

    kernel = find_linux_kernel()
    if not kernel:
        print("ERROR: Cannot find Linux kernel (vmlinuz). Install linux-image or set path.", file=sys.stderr)
        return 1

    # Create minimal initramfs with bench as /init
    with tempfile.TemporaryDirectory() as tmpdir:
        rootfs = os.path.join(tmpdir, "rootfs")
        for d in ("dev", "proc", "sys", "tmp"):
            os.makedirs(os.path.join(rootfs, d))
        shutil.copy2(str(BENCH_BIN), os.path.join(rootfs, "init"))
        os.chmod(os.path.join(rootfs, "init"), 0o755)

        initramfs = os.path.join(tmpdir, "initramfs.gz")
        r = subprocess.run(
            f"cd {rootfs} && find . -print0 | cpio --null -o --format=newc 2>/dev/null | gzip > {initramfs}",
            shell=True, capture_output=True,
        )
        if r.returncode != 0:
            print("Failed to create initramfs", file=sys.stderr)
            return 1

        rdinit_args = "-- --full" if full else ""
        cmd = [
            QEMU,
            "-kernel", str(kernel),
            "-initrd", initramfs,
            "-append", f"console=ttyS0 quiet panic=-1 rdinit=/init {rdinit_args}",
            "-m", "1024",
            "-nographic",
            "-no-reboot",
            "-cpu", "host",
            "--enable-kvm",
            "-mem-prealloc",
        ]

        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        try:
            for line in proc.stdout:
                line = line.rstrip("\n")
                print(line, flush=True)
                if "BENCH_END" in line:
                    proc.terminate()
                    break
        except KeyboardInterrupt:
            proc.terminate()
        finally:
            proc.wait(timeout=10)

    return 0


if __name__ == "__main__":
    sys.exit(main())
