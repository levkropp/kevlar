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
    "font-misc-misc",  # Basic X11 bitmap fonts (fixed, cursor)
    "font-cursor-misc",
    "font-dejavu",    # TrueType fonts for xterm/applications
    "fontconfig",     # Font configuration and fc-cache
    "mkfontscale",    # Generates fonts.dir and fonts.scale
    "xset",
    "xdpyinfo",
    "xsetroot",       # Set root window background color
    "xprop",          # X11 property inspection
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
            '    Option "ShadowFB" "off"\n'
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
            "# Set a visible background\n"
            "xsetroot -solid '#2E3440'\n"
            "# Launch xterm with a working font\n"
            "xterm -fa DejaVuSansMono -fs 11 -bg '#2E3440' -fg '#A3BE8C' &\n"
            "# Start twm\n"
            "exec twm\n"
        )
        os.chmod(root_home / ".xinitrc", 0o755)

        # twm config with visible colors and usable defaults
        (root_home / ".twmrc").write_text(
            'Color {\n'
            '    DefaultBackground "#3B4252"\n'
            '    DefaultForeground "#D8DEE9"\n'
            '    TitleBackground "#5E81AC"\n'
            '    TitleForeground "#ECEFF4"\n'
            '    MenuBackground "#3B4252"\n'
            '    MenuForeground "#E5E9F0"\n'
            '    MenuTitleBackground "#5E81AC"\n'
            '    MenuTitleForeground "#ECEFF4"\n'
            '    BorderColor "#81A1C1"\n'
            '    MenuShadowColor "#2E3440"\n'
            '}\n'
            'BorderWidth 2\n'
            'TitleFont "-misc-fixed-bold-r-normal--13-120-75-75-c-80-iso8859-1"\n'
            'MenuFont "-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso8859-1"\n'
            'IconManagerFont "-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso8859-1"\n'
            'ResizeFont "-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso8859-1"\n'
            '\n'
            '# Right-click menu\n'
            'Button3 = : root : f.menu "main"\n'
            'menu "main" {\n'
            '    "Kevlar Desktop"  f.title\n'
            '    "XTerm"           !"xterm -fa DejaVuSansMono -fs 11 -bg \'#2E3440\' -fg \'#A3BE8C\' &"\n'
            '    ""                f.nop\n'
            '    "Restart TWM"     f.restart\n'
            '    "Exit"            f.quit\n'
            '}\n'
        )

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

        # Generate proper font catalogs using mkfontdir/mkfontscale.
        # These must have correct XLFD entries for Xorg to serve fonts.
        print("  FONTS  Generating font catalogs...", file=sys.stderr)
        for font_dir_name in ["misc", "cursor", "TTF", "75dpi", "100dpi"]:
            font_dir = alpine_root / "usr" / "share" / "fonts" / font_dir_name
            if font_dir.exists():
                # Run mkfontscale and mkfontdir via the rootfs binaries
                mkfontscale = alpine_root / "usr" / "bin" / "mkfontscale"
                mkfontdir = alpine_root / "usr" / "bin" / "mkfontdir"
                if mkfontscale.exists():
                    subprocess.run(
                        [str(mkfontscale), str(font_dir)],
                        check=False, capture_output=True,
                    )
                if mkfontdir.exists():
                    subprocess.run(
                        [str(mkfontdir), str(font_dir)],
                        check=False, capture_output=True,
                    )
                # Verify
                fonts_dir = font_dir / "fonts.dir"
                if fonts_dir.exists():
                    with open(fonts_dir) as f:
                        count = f.readline().strip()
                    print(f"    {font_dir_name}: {count} fonts", file=sys.stderr)
                else:
                    print(f"    {font_dir_name}: no fonts.dir generated", file=sys.stderr)

        # Generate fontconfig cache
        fc_cache = alpine_root / "usr" / "bin" / "fc-cache"
        if fc_cache.exists():
            subprocess.run(
                [str(fc_cache), "-f", "-s", str(alpine_root / "usr" / "share" / "fonts")],
                check=False, capture_output=True,
                env={**os.environ, "FONTCONFIG_SYSROOT": str(alpine_root)},
            )

        # Install kxserver (minimal diagnostic X11 server) if it has been built.
        # See tools/kxserver/ and the Phase 0 plan in Documentation/blog/.
        # Built by `make kxserver-bin` (or `cd tools/kxserver && cargo build --release`).
        kxserver_bin = (
            ROOT
            / "tools"
            / "kxserver"
            / "target"
            / "x86_64-unknown-linux-musl"
            / "release"
            / "kxserver"
        )
        if kxserver_bin.exists():
            dst = alpine_root / "usr" / "bin" / "kxserver"
            shutil.copy2(kxserver_bin, dst)
            os.chmod(dst, 0o755)
            print(
                f"  KXSRV  installed {kxserver_bin.stat().st_size} bytes at /usr/bin/kxserver",
                file=sys.stderr,
            )
        else:
            print(
                "  KXSRV  no binary at tools/kxserver/target/.../release/kxserver (skip)",
                file=sys.stderr,
            )

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
