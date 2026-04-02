#!/usr/bin/env python3
"""Build an Alpine ext2 disk image with XFCE desktop pre-installed.

Creates a 1GB ext2 image with:
- Alpine minirootfs 3.21
- Xorg, xf86-video-fbdev, XFCE4, D-Bus, fonts, icons
- Pre-configured inittab, xinitrc, and xorg.conf for fbdev

Usage: python3 tools/build-alpine-xfce.py build/alpine-xfce.img
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

# Packages for XFCE desktop on fbdev
XFCE_PACKAGES = [
    # X11 server + fbdev driver
    "xorg-server",
    "xf86-video-fbdev",
    "xf86-input-libinput",
    "libinput",
    "xinit",
    "xauth",
    "xset",
    "xdpyinfo",
    # XFCE desktop
    "xfce4",
    "xfce4-terminal",
    # D-Bus (required by XFCE)
    "dbus",
    "dbus-x11",
    # Fonts (required by GTK)
    "font-dejavu",
    "font-misc-misc",
    "font-cursor-misc",
    "fontconfig",
    # Icons (required by XFCE panel/desktop)
    "adwaita-icon-theme",
    "hicolor-icon-theme",
    # Utilities
    "xterm",
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

    if os.path.exists(output):
        print(f"  SKIP  {output} already exists (delete to rebuild)", file=sys.stderr)
        return

    tarball = download(ALPINE_ROOTFS_URL,
                       CACHE / "alpine-minirootfs-3.21.3-x86_64.tar.gz")

    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir) / "alpine-root"
        root.mkdir()

        # Extract rootfs
        print("  EXTRACT  Alpine minirootfs", file=sys.stderr)
        with tarfile.open(tarball, "r:gz") as tar:
            try:
                tar.extractall(path=root, filter="fully_trusted")
            except TypeError:
                tar.extractall(path=root)

        # Configure apk repositories
        repos = root / "etc" / "apk" / "repositories"
        repos.parent.mkdir(parents=True, exist_ok=True)
        repos.write_text(
            "http://dl-cdn.alpinelinux.org/alpine/v3.21/main\n"
            "http://dl-cdn.alpinelinux.org/alpine/v3.21/community\n"
        )

        (root / "etc" / "resolv.conf").write_text("nameserver 10.0.2.3\n")
        (root / "var" / "cache" / "apk").mkdir(parents=True, exist_ok=True)
        (root / "tmp").mkdir(exist_ok=True)

        # Find apk binary
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
            print("  ERROR  No apk binary found.", file=sys.stderr)
            sys.exit(1)

        # Install XFCE packages. Post-install triggers fail (chroot not
        # available) — this is expected; the packages are still installed.
        print(f"  APK  Installing XFCE packages...", file=sys.stderr)
        result = subprocess.run(
            [apk_cmd, "--root", str(root), "--initdb",
             "--repositories-file", str(repos),
             "--allow-untrusted", "--no-cache",
             "add"] + XFCE_PACKAGES,
            text=True,
        )
        if result.returncode not in (0, 5):
            print(f"  ERROR  apk exited with {result.returncode}", file=sys.stderr)
            sys.exit(1)

        # --- Xorg config ---
        xorg_conf_dir = root / "etc" / "X11" / "xorg.conf.d"
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

        # --- xinitrc: start XFCE via dbus-launch ---
        home = root / "root"
        home.mkdir(exist_ok=True)
        (home / ".xinitrc").write_text(
            "#!/bin/sh\n"
            "# Start XFCE session via D-Bus\n"
            "exec dbus-launch --exit-with-session startxfce4\n"
        )
        os.chmod(home / ".xinitrc", 0o755)

        # --- inittab: minimal boot + shell (D-Bus started manually) ---
        (root / "etc" / "inittab").write_text(
            "# Kevlar XFCE inittab\n"
            "::sysinit:/bin/mkdir -p /run/dbus /tmp/.X11-unix\n"
            "::sysinit:/bin/hostname kevlar\n"
            "::sysinit:/usr/bin/dbus-daemon --system 2>/dev/null\n"
            "ttyS0::respawn:/sbin/getty -n -l /bin/sh -L 115200 ttyS0 vt100\n"
            "::shutdown:/bin/sync\n"
        )

        # --- Auto-start script for X + XFCE ---
        (root / "root" / "start-xfce.sh").write_text(
            "#!/bin/sh\n"
            "echo 'Starting Xorg + XFCE...'\n"
            "export HOME=/root\n"
            "export DISPLAY=:0\n"
            "# Start Xorg on VT1\n"
            "Xorg :0 -nolisten tcp vt1 &\n"
            "sleep 2\n"
            "# Start XFCE session\n"
            "dbus-launch startxfce4 &\n"
            "echo 'XFCE started. Use serial console for shell access.'\n"
        )
        os.chmod(root / "root" / "start-xfce.sh", 0o755)

        # --- Pre-create directories and users D-Bus needs ---
        # apk triggers can't run chroot, so create the messagebus user manually.
        (root / "run" / "dbus").mkdir(parents=True, exist_ok=True)
        (root / "var" / "run" / "dbus").mkdir(parents=True, exist_ok=True)

        # Append messagebus user/group to passwd/group/shadow if not present
        passwd = root / "etc" / "passwd"
        if "messagebus" not in passwd.read_text():
            with open(passwd, "a") as f:
                f.write("messagebus:x:100:101:D-Bus:/var/run/dbus:/sbin/nologin\n")
        group = root / "etc" / "group"
        if "messagebus" not in group.read_text():
            with open(group, "a") as f:
                f.write("messagebus:x:101:\n")
        shadow = root / "etc" / "shadow"
        if shadow.exists() and "messagebus" not in shadow.read_text():
            with open(shadow, "a") as f:
                f.write("messagebus:!::0:::::\n")

        # --- Pre-generate font directories for X11 ---
        for fd in [root / "usr" / "share" / "fonts" / d
                   for d in ("misc", "cursor", "dejavu", "TTF")]:
            if fd.exists():
                pcf_files = list(fd.glob("*.pcf.gz"))
                ttf_files = list(fd.glob("*.ttf"))
                all_fonts = pcf_files + ttf_files
                with open(fd / "fonts.dir", "w") as f:
                    f.write(f"{len(all_fonts)}\n")
                    for font in sorted(all_fonts):
                        f.write(f"{font.name} -misc-fixed-medium-r-normal--0-0-0-0-c-0-iso8859-1\n")

        # Create 1GB ext2 disk image
        size_mb = 1024
        print(f"  MKDISK  {size_mb}MB ext2", file=sys.stderr)
        subprocess.run(
            ["dd", "if=/dev/zero", f"of={output}",
             "bs=1M", f"count={size_mb}"],
            check=True, capture_output=True)
        subprocess.run(
            ["mke2fs", "-t", "ext2", "-d", str(root), output],
            check=True, capture_output=True)

    print(f"  DONE  {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
