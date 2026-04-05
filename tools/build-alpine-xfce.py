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
            '# Kevlar: disable udev auto-detect, use explicit fbdev config\n'
            'Section "ServerFlags"\n'
            '    Option "AutoAddDevices" "false"\n'
            '    Option "AutoAddGPU" "false"\n'
            'EndSection\n'
            '\n'
            'Section "Device"\n'
            '    Identifier "fbdev"\n'
            '    Driver "fbdev"\n'
            '    BusID "PCI:0:2:0"\n'
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
            "::sysinit:/bin/rm -f /run/dbus/dbus.pid\n"
            "::sysinit:/bin/hostname kevlar\n"
            "::sysinit:/usr/bin/dbus-uuidgen --ensure 2>/dev/null\n"
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
            "# Clean up stale state\n"
            "rm -f /tmp/.X0-lock /tmp/.X11-unix/X0\n"
            "# Start Xorg on VT1 (Alpine installs as /usr/libexec/Xorg)\n"
            "/usr/libexec/Xorg :0 -nolisten tcp -noreset "
            "-config /etc/X11/xorg.conf.d/10-fbdev.conf vt1 &\n"
            "sleep 3\n"
            "# Start XFCE session via D-Bus\n"
            "dbus-launch --exit-with-session startxfce4 &\n"
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

        # --- GDK-Pixbuf loaders cache (critical for GTK rendering) ---
        # Without this, GTK3 can't load ANY images → desktop renders as black.
        loaders_dir = root / "usr" / "lib" / "gdk-pixbuf-2.0" / "2.10.0"
        loaders_dir.mkdir(parents=True, exist_ok=True)
        loaders_cache = loaders_dir / "loaders.cache"
        # Find actual loader modules
        loader_so_dir = loaders_dir / "loaders"
        modules = sorted(loader_so_dir.glob("*.so")) if loader_so_dir.exists() else []
        # Write a cache that registers both built-in formats and module loaders.
        with open(loaders_cache, "w") as f:
            f.write("# GDK-Pixbuf image loader modules cache (pre-generated by Kevlar)\n")
            f.write("# Auto-generated, do not edit\n\n")
            for mod in modules:
                rel_path = "/" + str(mod.relative_to(root))
                name = mod.stem.replace("libpixbufloader-", "").replace("libpixbufloader_", "")
                f.write(f'"{rel_path}"\n')
                f.write(f'"{name}" 5 "gdk-pixbuf" "{name} image" "LGPL"\n')
                # Map module names to MIME types
                mime_map = {
                    "svg": "image/svg+xml", "png": "image/png",
                    "jpeg": "image/jpeg", "gif": "image/gif",
                    "bmp": "image/bmp", "tiff": "image/tiff",
                    "ico": "image/x-icon", "xpm": "image/x-xpixmap",
                    "xbm": "image/x-xbitmap", "tga": "image/x-tga",
                    "ani": "application/x-navi-animation",
                    "icns": "image/x-icns", "pnm": "image/x-portable-anymap",
                    "qtif": "image/x-quicktime",
                }
                mime = mime_map.get(name, f"image/{name}")
                ext = name[:4]
                f.write(f'"{mime}" "{ext}" ""\n\n')
        print(f"  CACHE  loaders.cache: {len(modules)} modules", file=sys.stderr)

        # --- Fontconfig cache (host-side if available) ---
        fc_cache = shutil.which("fc-cache")
        if fc_cache:
            try:
                subprocess.run([fc_cache, "--sysroot", str(root), "-f"],
                              capture_output=True, timeout=30)
                print("  CACHE  fc-cache: generated", file=sys.stderr)
            except Exception:
                print("  CACHE  fc-cache: skipped", file=sys.stderr)

        # --- MIME database cache ---
        update_mime = shutil.which("update-mime-database")
        if update_mime:
            mime_dir = root / "usr" / "share" / "mime"
            if mime_dir.exists():
                try:
                    subprocess.run([update_mime, str(mime_dir)],
                                  capture_output=True, timeout=30)
                    print("  CACHE  mime.cache: generated", file=sys.stderr)
                except Exception:
                    pass

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
