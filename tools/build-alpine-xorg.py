#!/usr/bin/env python3
"""Build an Alpine ext2 disk image with Xorg pre-installed.

Creates a 512MB ext2 image with:
- Alpine minirootfs 3.21
- xorg-server, xf86-video-fbdev, xterm, twm, xinit
- Configured for fbdev on /dev/fb0

Usage: python3 tools/build-alpine-xorg.py build/alpine-xorg.img
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

# Packages to install for a minimal X11 setup
XORG_PACKAGES = [
    "xorg-server",
    "xf86-video-fbdev",
    "xf86-input-libinput",
    "libinput",
    "xterm",
    "twm",            # Tiny window manager
    "xinit",
    "xauth",
    "font-misc-misc",  # Basic X11 fonts
    "font-cursor-misc",
    "xset",
    "xdpyinfo",
]


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

    # Check if output already exists
    if os.path.exists(output):
        print(f"  SKIP  {output} already exists (delete to rebuild)", file=sys.stderr)
        return

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

        # Configure apk repositories (main + community for X11)
        repos = alpine_root / "etc" / "apk" / "repositories"
        repos.parent.mkdir(parents=True, exist_ok=True)
        repos.write_text(
            "http://dl-cdn.alpinelinux.org/alpine/v3.21/main\n"
            "http://dl-cdn.alpinelinux.org/alpine/v3.21/community\n"
        )

        resolv = alpine_root / "etc" / "resolv.conf"
        resolv.write_text("nameserver 10.0.2.3\n")

        (alpine_root / "var" / "cache" / "apk").mkdir(parents=True, exist_ok=True)
        (alpine_root / "tmp").mkdir(exist_ok=True)

        # Find apk binary (system apk or cached apk.static)
        apk_cmd = shutil.which("apk")
        if not apk_cmd:
            for candidate in [
                ROOT / "build" / "native-cache" / "alpine-pkgs" / "apk-tools-static" / "sbin" / "apk.static",
                ROOT / "build" / "native-cache" / "ext-bin" / "apk.static",
            ]:
                if candidate.exists():
                    apk_cmd = str(candidate)
                    break

        if not apk_cmd:
            print("  ERROR  No apk binary found. Run `make build` first to download apk.static.", file=sys.stderr)
            sys.exit(1)

        # Install X11 packages using apk with --root
        print(f"  APK  Installing X11 packages into rootfs using {Path(apk_cmd).name}...", file=sys.stderr)
        subprocess.run(
            [apk_cmd, "--root", str(alpine_root), "--initdb",
             "--repositories-file", str(repos),
             "--allow-untrusted", "--no-cache",
             "add"] + XORG_PACKAGES,
            check=True, text=True,
        )

        # Configure xorg.conf for fbdev
        xorg_conf_dir = alpine_root / "etc" / "X11" / "xorg.conf.d"
        xorg_conf_dir.mkdir(parents=True, exist_ok=True)
        (xorg_conf_dir / "10-fbdev.conf").write_text(
            'Section "Device"\n'
            '    Identifier "fbdev"\n'
            '    Driver "fbdev"\n'
            '    Option "fbdev" "/dev/fb0"\n'
            'EndSection\n'
            '\n'
            'Section "Screen"\n'
            '    Identifier "default"\n'
            '    Device "fbdev"\n'
            '    DefaultDepth 24\n'
            '    SubSection "Display"\n'
            '        Depth 24\n'
            '        Modes "1024x768"\n'
            '    EndSubSection\n'
            'EndSection\n'
        )

        # Create .xinitrc for a basic X11 session
        root_home = alpine_root / "root"
        root_home.mkdir(exist_ok=True)
        (root_home / ".xinitrc").write_text(
            "#!/bin/sh\n"
            "xterm &\n"
            "exec twm\n"
        )
        os.chmod(root_home / ".xinitrc", 0o755)

        # Create a first-boot script that generates font caches
        (root_home / "setup-fonts.sh").write_text(
            "#!/bin/sh\n"
            "echo 'Generating font caches...'\n"
            "for dir in /usr/share/fonts/*; do\n"
            "  [ -d \"$dir\" ] && mkfontscale \"$dir\" 2>/dev/null && mkfontdir \"$dir\" 2>/dev/null\n"
            "done\n"
            "fc-cache -f 2>/dev/null\n"
            "echo 'Font caches generated.'\n"
        )
        os.chmod(root_home / "setup-fonts.sh", 0o755)

        # Pre-create basic fonts.dir so X11 can find at least some fonts
        font_dirs = [
            alpine_root / "usr" / "share" / "fonts" / "misc",
            alpine_root / "usr" / "share" / "fonts" / "cursor",
        ]
        for fd in font_dirs:
            if fd.exists():
                fonts_dir = fd / "fonts.dir"
                # Count .pcf.gz files
                pcf_files = list(fd.glob("*.pcf.gz"))
                with open(fonts_dir, "w") as f:
                    f.write(f"{len(pcf_files)}\n")
                    for pcf in sorted(pcf_files):
                        # Minimal fonts.dir entry: filename -misc-fixed-medium-r-...
                        f.write(f"{pcf.name} -misc-fixed-medium-r-normal--0-0-0-0-c-0-iso8859-1\n")

        # Create 512MB ext2 disk image
        size_mb = 512
        print(f"  MKDISK  {size_mb}MB ext2", file=sys.stderr)
        subprocess.run(
            ["dd", "if=/dev/zero", f"of={output}",
             "bs=1M", f"count={size_mb}"],
            check=True, capture_output=True)
        subprocess.run(
            ["mke2fs", "-t", "ext2", "-d", str(alpine_root), output],
            check=True, capture_output=True)

    print(f"  DONE  {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
