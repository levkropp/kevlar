#!/usr/bin/env python3
"""Build full Alpine ext4 disk image natively (no Docker required).

Downloads Alpine minirootfs, installs OpenRC + build-base via apk.static,
and creates a 512MB ext4 disk image.

Usage: python3 tools/build-alpine-full.py build/alpine.img
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

        # Configure apk repositories (HTTP, not HTTPS)
        repos = alpine_root / "etc" / "apk" / "repositories"
        repos.parent.mkdir(parents=True, exist_ok=True)
        repos.write_text(
            "http://dl-cdn.alpinelinux.org/alpine/v3.21/main\n"
            "http://dl-cdn.alpinelinux.org/alpine/v3.21/community\n"
        )

        # Network config
        resolv = alpine_root / "etc" / "resolv.conf"
        resolv.write_text("nameserver 10.0.2.3\n")

        # Hostname
        (alpine_root / "etc" / "hostname").write_text("kevlar\n")

        # Timezone
        (alpine_root / "etc" / "TZ").write_text("UTC0\n")

        # Root login without password
        shadow = alpine_root / "etc" / "shadow"
        if shadow.exists():
            text = shadow.read_text()
            text = text.replace("root:*:", "root::")
            shadow.write_text(text)

        # Console
        securetty = alpine_root / "etc" / "securetty"
        if securetty.exists():
            text = securetty.read_text()
            if "ttyS0" not in text:
                securetty.write_text(text + "ttyS0\n")

        # ld-musl path
        (alpine_root / "etc" / "ld-musl-x86_64.path").write_text("/lib\n/usr/lib\n")

        # Cache dirs
        (alpine_root / "var" / "cache" / "apk").mkdir(parents=True, exist_ok=True)
        (alpine_root / "tmp").mkdir(mode=0o1777, exist_ok=True)

        # Inittab for BusyBox init
        inittab = (
            "::sysinit:/sbin/ip link set lo up\n"
            "::sysinit:/sbin/ip link set eth0 up\n"
            "::sysinit:/sbin/ip addr add 10.0.2.15/24 dev eth0\n"
            "::sysinit:/sbin/ip route add default via 10.0.2.2\n"
            "::sysinit:/sbin/openrc sysinit\n"
            "::sysinit:/sbin/openrc boot\n"
            "::wait:/sbin/openrc default\n"
            "ttyS0::respawn:/sbin/getty -n -l /bin/sh -L 115200 ttyS0 vt100\n"
            "::ctrlaltdel:/sbin/reboot\n"
            "::shutdown:/sbin/openrc shutdown\n"
        )
        (alpine_root / "etc" / "inittab").write_text(inittab)

        # Pre-install GCC toolchain using apk.static from the initramfs
        apk_static = ROOT / "build" / "initramfs-rootfs" / "bin" / "apk.static"
        if apk_static.exists():
            print("  APK     pre-installing gcc musl-dev make", file=sys.stderr)
            try:
                subprocess.run(
                    [str(apk_static), "--root", str(alpine_root),
                     "--initdb", "--allow-untrusted",
                     "--repositories-file", str(alpine_root / "etc" / "apk" / "repositories"),
                     "add", "gcc", "musl-dev", "make"],
                    check=True, capture_output=True, timeout=120)
                print("  APK     gcc installed OK", file=sys.stderr)
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
                print(f"  APK     gcc install failed (non-fatal): {e}", file=sys.stderr)

        # Create libgcc.a symlink in /usr/lib (GCC's linker can't find it otherwise)
        gcc_lib = alpine_root / "usr" / "lib" / "gcc" / "x86_64-alpine-linux-musl"
        if gcc_lib.exists():
            for ver_dir in gcc_lib.iterdir():
                libgcc_a = ver_dir / "libgcc.a"
                if libgcc_a.exists():
                    target = alpine_root / "usr" / "lib" / "libgcc.a"
                    if not target.exists():
                        target.symlink_to(str(libgcc_a).replace(str(alpine_root), ""))

        # Symlink /usr/lib shared libraries into /lib for musl's dynamic linker
        usr_lib = alpine_root / "usr" / "lib"
        lib_dir = alpine_root / "lib"
        if usr_lib.exists():
            for f in usr_lib.glob("lib*.so*"):
                target = lib_dir / f.name
                if not target.exists():
                    target.symlink_to(f"/usr/lib/{f.name}")

        # Create 1GB ext4 disk image (GCC + curl + python3 need ~300MB)
        print("  MKDISK  1GB ext4", file=sys.stderr)
        subprocess.run(
            ["dd", "if=/dev/zero", f"of={output}",
             "bs=1M", "count=1024"],
            check=True, capture_output=True)
        subprocess.run(
            ["mke2fs", "-t", "ext4", "-q", "-d", str(alpine_root), output],
            check=True, capture_output=True)

    print(f"  DONE  {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
