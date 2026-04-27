#!/usr/bin/env python3
"""Build an Alpine ext2 disk image with an LXDE-style lightweight desktop.

Alpine 3.21 doesn't ship the original LXDE (lxsession/lxpanel); the
closest equivalent it provides is openbox + tint2 + pcmanfm, which is
the same architecture — independent WM + panel + file-manager-as-desktop
— without the lxsession meta-binary.  This script installs those and
starts them via a small openbox autostart script so the boot flow
matches what LXDE would do.

Built via `apko` so the script runs cross-arch on macOS host —
matches the openbox build (tools/build-alpine-openbox.py).

Usage:
    python3 tools/build-alpine-lxde.py [--arch aarch64|x86_64] \\
        build/alpine-lxde.aarch64.img
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

# Packages for the LXDE-style lightweight desktop on fbdev.  Openbox
# plays the WM role, tint2 the panel, pcmanfm renders the desktop
# wallpaper + icons.  feh handles wallpaper set-and-forget.  This
# combination is what upstream Alpine users deploy when they want "LXDE
# but without GNOME/Qt baggage."
LXDE_PACKAGES = [
    "alpine-baselayout",
    "busybox",
    "musl-utils",
    # X11 server + fbdev driver
    "xorg-server",
    "xf86-video-fbdev",
    "xf86-input-evdev",
    "xinit",
    "xauth",
    "xset",
    "xsetroot",
    "xdpyinfo",
    "xprop",
    # LXDE-style stack
    "openbox",          # window manager
    "tint2",            # panel
    "pcmanfm",          # file manager + desktop renderer
    "feh",              # wallpaper setter
    # D-Bus
    "dbus",
    "dbus-x11",
    # Fonts (required by GTK)
    "font-dejavu",
    "font-misc-misc",
    "font-cursor-misc",
    "fontconfig",
    # GTK icon themes
    "adwaita-icon-theme",
    "hicolor-icon-theme",
    # Utilities
    "xterm",
]


def apko_yaml(arch: str) -> str:
    return (
        "contents:\n"
        "  repositories:\n"
        + "".join(f"    - {r}\n" for r in ALPINE_REPOS) +
        f"  keyring:\n    - {ALPINE_KEY}\n"
        "  packages:\n"
        + "".join(f"    - {p}\n" for p in LXDE_PACKAGES) +
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
        print(f"  APKO  resolving + installing {len(LXDE_PACKAGES)} packages "
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
        if not (root / "usr" / "bin" / "openbox").exists():
            print(f"  ERROR  apko did not install openbox; rootfs at {root}",
                  file=sys.stderr)
            sys.exit(1)

        # Xorg fbdev config — uses Kevlar's /dev/fb0 (ramfb).
        # ShadowFB defaults to "on"; the arm64 fb mmap fix in blog 245
        # (Normal-NC vs Device-nGnRnE) makes that path work.  No BusID
        # because we don't have PCI on virt; fbdev driver finds /dev/fb0
        # directly.
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

        home = root / "root"
        home.mkdir(exist_ok=True)
        # openbox autostart: tint2 + pcmanfm --desktop + a wallpaper.
        # This is what `startlxde` does on Alpine-Ubuntu-equivalent systems.
        ob_auto_dir = home / ".config" / "openbox"
        ob_auto_dir.mkdir(parents=True, exist_ok=True)
        (ob_auto_dir / "autostart").write_text(
            "#!/bin/sh\n"
            "# Openbox autostart — LXDE-style session.\n"
            "tint2 &\n"
            "xsetroot -solid '#2d5577' &\n"
            "pcmanfm --desktop &\n"
        )
        os.chmod(ob_auto_dir / "autostart", 0o755)

        # pcmanfm desktop config — tell it to draw a solid-color
        # background rather than black (default when no wallpaper set).
        # Also disable the "Templates missing" dialog and keep icons
        # visible so the test's pixel-visibility check picks up color.
        #
        # IMPORTANT: pcmanfm with `--desktop` (no --profile) uses the
        # "default" profile, NOT "LXDE".  Earlier versions of this
        # script wrote the config under .../pcmanfm/LXDE/ which pcmanfm
        # then ignored, falling back to a black wallpaper — the test
        # then had to kill pcmanfm and use xsetroot as a workaround,
        # which was racy.  Write under "default" so pcmanfm reads it
        # without --profile.
        pcmanfm_conf = (
            "[config]\n"
            "bm_open_method=0\n"
            "\n"
            "[volume]\n"
            "mount_on_startup=0\n"
            "mount_removable=0\n"
            "autorun=0\n"
            "\n"
            "[ui]\n"
            "always_show_tabs=0\n"
            "max_tab_chars=32\n"
            "\n"
            "[desktop]\n"
            "wallpaper_mode=color\n"
            "desktop_bg=#336699\n"
            "desktop_fg=#ffffff\n"
            "desktop_shadow=#000000\n"
            "show_wm_menu=0\n"
            "sort=mtime;ascending;\n"
            "show_documents=0\n"
            "show_trash=0\n"
            "show_mounts=0\n"
        )
        for profile in ("default", "LXDE"):
            d = home / ".config" / "pcmanfm" / profile
            d.mkdir(parents=True, exist_ok=True)
            (d / "pcmanfm.conf").write_text(pcmanfm_conf)
        # .xinitrc starts Openbox directly — no lxsession needed.
        (home / ".xinitrc").write_text(
            "#!/bin/sh\n"
            "exec dbus-launch --exit-with-session openbox-session\n"
        )
        os.chmod(home / ".xinitrc", 0o755)

        (root / "etc" / "inittab").write_text(
            "# Kevlar LXDE inittab\n"
            "::sysinit:/bin/mkdir -p /run/dbus /tmp/.X11-unix\n"
            "::sysinit:/bin/rm -f /run/dbus/dbus.pid\n"
            "::sysinit:/bin/hostname kevlar\n"
            "::sysinit:/usr/bin/dbus-uuidgen --ensure 2>/dev/null\n"
            "::sysinit:/usr/bin/dbus-daemon --system 2>/dev/null\n"
            # Auto-start LXDE session on boot (for `make run-alpine-lxde`).
            # Output goes to /var/log/lxde-session.log so it doesn't
            # interleave with the serial console.
            "::once:/bin/sh /root/start-lxde.sh > /var/log/lxde-session.log 2>&1\n"
            "ttyS0::respawn:/sbin/getty -n -l /bin/sh -L 115200 ttyS0 vt100\n"
            "::shutdown:/bin/sync\n"
        )

        # Auto-start script — runs Openbox (which sources autostart for
        # tint2 + pcmanfm + xsetroot) on top of Xorg.  Uses absolute paths
        # because init's PATH is minimal.  Logs progress so failures are
        # visible in /var/log/lxde-session.log.
        (root / "root" / "start-lxde.sh").write_text(
            "#!/bin/sh\n"
            "set -x\n"
            "export PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin\n"
            "export HOME=/root\n"
            "export DISPLAY=:0\n"
            "echo 'start-lxde: launching Xorg on vt1'\n"
            "rm -f /tmp/.X0-lock /tmp/.X11-unix/X0\n"
            "/usr/libexec/Xorg :0 -nolisten tcp -noreset \\\n"
            "    -config /etc/X11/xorg.conf.d/10-fbdev.conf vt1 \\\n"
            "    >/var/log/Xorg.0.log 2>&1 &\n"
            "XORG_PID=$!\n"
            "echo \"start-lxde: Xorg pid=$XORG_PID, sleeping 3s for socket\"\n"
            "sleep 3\n"
            "if [ ! -S /tmp/.X11-unix/X0 ]; then\n"
            "    echo 'start-lxde: ERROR /tmp/.X11-unix/X0 missing — Xorg failed'\n"
            "    cat /var/log/Xorg.0.log | tail -30\n"
            "    exit 1\n"
            "fi\n"
            "echo 'start-lxde: launching openbox (autostart runs tint2 + pcmanfm)'\n"
            "/usr/bin/openbox >/var/log/openbox.log 2>&1 &\n"
            "OB_PID=$!\n"
            "echo \"start-lxde: openbox pid=$OB_PID — desktop should appear in QEMU window\"\n"
            "wait $OB_PID\n"
            "echo \"start-lxde: openbox exited (rc=$?)\"\n"
        )
        os.chmod(root / "root" / "start-lxde.sh", 0o755)

        # D-Bus messagebus user/group (same as XFCE)
        (root / "run" / "dbus").mkdir(parents=True, exist_ok=True)
        (root / "var" / "run" / "dbus").mkdir(parents=True, exist_ok=True)

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

        # Pre-generate X11 font directories with proper XLFD names (reused
        # from XFCE — XCB/Xorg won't start without these on fbdev).
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
                    if "iso10646-1" in xlfd:
                        entries.append(f"{pcf.name} {xlfd.replace('iso10646-1', 'iso8859-1')}")
                else:
                    entries.append(f"{pcf.name} -misc-fixed-medium-r-normal--0-0-0-0-c-0-iso8859-1")
            with open(misc_dir / "fonts.dir", "w") as f:
                f.write(f"{len(entries)}\n")
                for e in entries:
                    f.write(e + "\n")
            print(f"  FONTS  fonts.dir: {len(entries)} entries", file=sys.stderr)

        # GDK-Pixbuf loaders cache (shared shape with XFCE).
        loaders_dir = root / "usr" / "lib" / "gdk-pixbuf-2.0" / "2.10.0"
        loaders_dir.mkdir(parents=True, exist_ok=True)
        loaders_cache = loaders_dir / "loaders.cache"
        loader_so_dir = loaders_dir / "loaders"
        modules = sorted(loader_so_dir.glob("*.so")) if loader_so_dir.exists() else []
        with open(loaders_cache, "w") as f:
            f.write("# GDK-Pixbuf image loader modules cache (pre-generated by Kevlar)\n")
            f.write("# Auto-generated, do not edit\n\n")
            # Built-in PNG/JPEG (statically linked into libgdk_pixbuf) —
            # LXDE's lxpanel uses both heavily.
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

        fc_cache = shutil.which("fc-cache")
        if fc_cache:
            try:
                subprocess.run([fc_cache, "--sysroot", str(root), "-f"],
                              capture_output=True, timeout=30)
                print("  CACHE  fc-cache: generated", file=sys.stderr)
            except Exception:
                pass

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
