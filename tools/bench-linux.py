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
        Path(f"/usr/lib/modules/{release}/vmlinuz"),
        Path(f"/boot/vmlinuz-{release}"),
        Path("/boot/vmlinuz"),
    ]
    for p in candidates:
        if p.exists():
            return p
    # Fallback: any vmlinuz under /usr/lib/modules (Arch UKI installs).
    import glob
    for g in sorted(glob.glob("/usr/lib/modules/*/vmlinuz"), reverse=True):
        return Path(g)
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

    # Create initramfs with bench as /init and BusyBox for workload benchmarks.
    # Without BusyBox, shell_noop/pipe_grep/sed_pipeline would exec-fail
    # immediately (no /bin/sh) and measure only fork+_exit(127) — not the
    # actual workload.  Including the same BusyBox used by Kevlar ensures a
    # fair apples-to-apples comparison.
    busybox_bin = ROOT / "build" / "native-cache" / "ext-bin" / "busybox"
    if not busybox_bin.exists():
        print(f"WARNING: {busybox_bin} not found — workload benchmarks will "
              "fail to exec (build Kevlar first to compile BusyBox).",
              file=sys.stderr)

    with tempfile.TemporaryDirectory() as tmpdir:
        rootfs = os.path.join(tmpdir, "rootfs")
        for d in ("bin", "sbin", "usr/bin", "usr/sbin",
                  "dev", "proc", "sys", "tmp"):
            os.makedirs(os.path.join(rootfs, d))
        shutil.copy2(str(BENCH_BIN), os.path.join(rootfs, "init"))
        os.chmod(os.path.join(rootfs, "init"), 0o755)

        # Install BusyBox + applet symlinks so workload benchmarks work.
        if busybox_bin.exists():
            bb_dest = os.path.join(rootfs, "bin", "busybox")
            shutil.copy2(str(busybox_bin), bb_dest)
            os.chmod(bb_dest, 0o755)
            r = subprocess.run(
                [bb_dest, "--list-full"],
                capture_output=True, text=True,
            )
            for line in r.stdout.strip().split("\n"):
                applet = line.strip()
                if not applet:
                    continue
                dest = os.path.join(rootfs, applet)
                os.makedirs(os.path.dirname(dest), exist_ok=True)
                if not os.path.exists(dest):
                    os.symlink("/bin/busybox", dest)

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
