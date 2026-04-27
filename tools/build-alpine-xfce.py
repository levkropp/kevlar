#!/usr/bin/env python3
"""Build an Alpine ext2 disk image with XFCE desktop pre-installed.

Creates a 1GB ext2 image with:
- Alpine minirootfs 3.21
- Xorg, xf86-video-fbdev, XFCE4, D-Bus, fonts, icons
- Pre-configured inittab, xinitrc, and xorg.conf for fbdev

Built via `apko` so the script runs cross-arch on macOS host —
matches the openbox / lxde build pattern.

Usage:
    python3 tools/build-alpine-xfce.py [--arch aarch64|x86_64] \\
        build/alpine-xfce.aarch64.img
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

ALPINE_REPOS = [
    "https://dl-cdn.alpinelinux.org/alpine/v3.21/main",
    "https://dl-cdn.alpinelinux.org/alpine/v3.21/community",
]
ALPINE_KEY = (
    "https://alpinelinux.org/keys/"
    "alpine-devel@lists.alpinelinux.org-6165ee59.rsa.pub"
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
    "xsetroot",
    "xprop",
]


def apko_yaml(arch: str) -> str:
    return (
        "contents:\n"
        "  repositories:\n"
        + "".join(f"    - {r}\n" for r in ALPINE_REPOS) +
        f"  keyring:\n    - {ALPINE_KEY}\n"
        "  packages:\n"
        + "".join(f"    - {p}\n" for p in XFCE_PACKAGES) +
        "\n"
        "archs:\n"
        f"  - {arch}\n"
        "\n"
        "cmd: /sbin/init\n"
    )


def ensure_tool(name: str, brew_hint: str | None = None) -> str:
    path = shutil.which(name)
    if path:
        return path
    for p in ("/opt/homebrew/opt/e2fsprogs/sbin", "/usr/local/opt/e2fsprogs/sbin"):
        cand = Path(p) / name
        if cand.exists():
            return str(cand)
    hint = f" (install with `brew install {brew_hint}`)" if brew_hint else ""
    print(f"  ERROR  `{name}` not found in PATH{hint}", file=sys.stderr)
    sys.exit(1)


def build_rootfs_with_apko(arch: str, out_dir: Path) -> None:
    apko = ensure_tool("apko", "apko")
    with tempfile.TemporaryDirectory() as ytmp:
        yaml_path = Path(ytmp) / "apko.yaml"
        yaml_path.write_text(apko_yaml(arch))
        tgz_path = Path(ytmp) / "rootfs.tar.gz"
        print(f"  APKO  resolving + installing {len(XFCE_PACKAGES)} packages "
              f"({arch})...", file=sys.stderr)
        r = subprocess.run(
            [apko, "build-minirootfs",
             "--ignore-signatures",
             str(yaml_path), str(tgz_path)],
            text=True, capture_output=True,
        )
        if not tgz_path.exists() or tgz_path.stat().st_size < 1024:
            print(r.stdout, file=sys.stderr)
            print(r.stderr, file=sys.stderr)
            print("  ERROR  apko did not produce a rootfs", file=sys.stderr)
            sys.exit(1)
        print(f"  EXTRACT  rootfs ({tgz_path.stat().st_size // (1024*1024)}MB)",
              file=sys.stderr)
        subprocess.run(
            ["tar", "-xzf", str(tgz_path), "-C", str(out_dir)],
            capture_output=True,
        )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arch", default="aarch64",
                    choices=["aarch64", "x86_64"])
    ap.add_argument("output", help="Path to the output .img file")
    args = ap.parse_args()

    output = args.output

    if os.path.exists(output):
        print(f"  SKIP  {output} already exists (delete to rebuild)", file=sys.stderr)
        return

    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir) / "alpine-root"
        root.mkdir()

        build_rootfs_with_apko(args.arch, root)
        if not (root / "usr" / "bin" / "xfce4-session").exists() \
            and not (root / "usr" / "bin" / "startxfce4").exists():
            print("  ERROR  apko did not install XFCE; rootfs at " + str(root),
                  file=sys.stderr)
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

        # Append messagebus user/group to passwd/group/shadow if not present.
        # apko's minirootfs is missing some of these files entirely — create
        # them with a sensible default if so.
        passwd = root / "etc" / "passwd"
        if not passwd.exists():
            passwd.parent.mkdir(parents=True, exist_ok=True)
            passwd.write_text("root:x:0:0:root:/root:/bin/sh\n")
        if "messagebus" not in passwd.read_text():
            with open(passwd, "a") as f:
                f.write("messagebus:x:100:101:D-Bus:/var/run/dbus:/sbin/nologin\n")
        group = root / "etc" / "group"
        if not group.exists():
            group.write_text("root:x:0:\n")
        if "messagebus" not in group.read_text():
            with open(group, "a") as f:
                f.write("messagebus:x:101:\n")
        shadow = root / "etc" / "shadow"
        if shadow.exists() and "messagebus" not in shadow.read_text():
            with open(shadow, "a") as f:
                f.write("messagebus:!::0:::::\n")

        # --- Pre-generate X11 font directories with proper XLFD names ---
        # mkfontdir inside QEMU fails to parse PCF properties, so we use
        # known XLFD mappings for the standard misc-fixed fonts.
        KNOWN_XLFD = {
            "6x13.pcf.gz": "-misc-fixed-medium-r-semicondensed--13-120-75-75-c-60-iso10646-1",
            "6x13B.pcf.gz": "-misc-fixed-bold-r-semicondensed--13-120-75-75-c-60-iso10646-1",
            "6x13O.pcf.gz": "-misc-fixed-medium-o-semicondensed--13-120-75-75-c-60-iso10646-1",
            "7x13.pcf.gz": "-misc-fixed-medium-r-normal--13-120-75-75-c-70-iso10646-1",
            "7x13B.pcf.gz": "-misc-fixed-bold-r-normal--13-120-75-75-c-70-iso10646-1",
            "7x14.pcf.gz": "-misc-fixed-medium-r-normal--14-130-75-75-c-70-iso10646-1",
            "7x14B.pcf.gz": "-misc-fixed-bold-r-normal--14-130-75-75-c-70-iso10646-1",
            "8x13.pcf.gz": "-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso10646-1",
            "8x13B.pcf.gz": "-misc-fixed-bold-r-normal--13-120-75-75-c-80-iso10646-1",
            "8x13O.pcf.gz": "-misc-fixed-medium-o-normal--13-120-75-75-c-80-iso10646-1",
            "9x15.pcf.gz": "-misc-fixed-medium-r-normal--15-140-75-75-c-90-iso10646-1",
            "9x15B.pcf.gz": "-misc-fixed-bold-r-normal--15-140-75-75-c-90-iso10646-1",
            "9x18.pcf.gz": "-misc-fixed-medium-r-normal--18-120-100-100-c-90-iso10646-1",
            "9x18B.pcf.gz": "-misc-fixed-bold-r-normal--18-120-100-100-c-90-iso10646-1",
            "10x20.pcf.gz": "-misc-fixed-medium-r-normal--20-200-75-75-c-100-iso10646-1",
            "5x7.pcf.gz": "-misc-fixed-medium-r-normal--7-70-75-75-c-50-iso10646-1",
            "5x8.pcf.gz": "-misc-fixed-medium-r-normal--8-80-75-75-c-50-iso10646-1",
            "4x6.pcf.gz": "-misc-fixed-medium-r-normal--6-60-75-75-c-40-iso10646-1",
            "6x10.pcf.gz": "-misc-fixed-medium-r-normal--10-100-75-75-c-60-iso10646-1",
            "6x12.pcf.gz": "-misc-fixed-medium-r-semicondensed--12-110-75-75-c-60-iso10646-1",
            "6x9.pcf.gz": "-misc-fixed-medium-r-normal--9-90-75-75-c-60-iso10646-1",
        }
        misc_dir = root / "usr" / "share" / "fonts" / "misc"
        if misc_dir.exists():
            entries = []
            for pcf in sorted(misc_dir.glob("*.pcf.gz")):
                xlfd = KNOWN_XLFD.get(pcf.name)
                if xlfd:
                    entries.append(f"{pcf.name} {xlfd}")
                    # Also add ISO-8859-1 alias for each ISO-10646-1 font
                    if "iso10646-1" in xlfd:
                        entries.append(f"{pcf.name} {xlfd.replace('iso10646-1', 'iso8859-1')}")
                else:
                    entries.append(f"{pcf.name} -misc-fixed-medium-r-normal--0-0-0-0-c-0-iso8859-1")
            with open(misc_dir / "fonts.dir", "w") as f:
                f.write(f"{len(entries)}\n")
                for e in entries:
                    f.write(e + "\n")
            print(f"  FONTS  fonts.dir: {len(entries)} entries ({len(KNOWN_XLFD)} with XLFD)", file=sys.stderr)

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

            # Alpine's gdk-pixbuf builds PNG + JPEG as BUILTIN (statically
            # linked into libgdk_pixbuf-2.0.so, verified via
            # `strings libgdk_pixbuf-2.0.so.0 | grep gdk_pixbuf__png`).
            # There's no separate .so for them, but the cache still needs
            # explicit entries or gdk_pixbuf_loader_new_with_type("png")
            # returns NULL — xfdesktop then passes NULL to every subsequent
            # gdk_pixbuf_* and hits Wnck assertions → SIGABRT.
            # An empty first line (instead of a module path) marks the
            # loader as built-in.
            f.write('""\n')
            f.write('"png" 6 "gdk-pixbuf" "The PNG image format" "LGPL"\n')
            f.write('"image/png" "png" ""\n\n')
            f.write('""\n')
            f.write('"jpeg" 5 "gdk-pixbuf" "The JPEG image format" "LGPL"\n')
            f.write('"image/jpeg" "jpeg" ""\n\n')

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
        mke2fs = ensure_tool("mke2fs", "e2fsprogs")
        size_mb = 1024
        print(f"  MKDISK  {size_mb}MB ext2", file=sys.stderr)
        subprocess.run(
            ["dd", "if=/dev/zero", f"of={output}",
             "bs=1M", f"count={size_mb}"],
            check=True, capture_output=True)
        subprocess.run(
            [mke2fs, "-t", "ext2", "-d", str(root), "-F", output],
            check=True, capture_output=True)

    print(f"  DONE  {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
