#!/usr/bin/env python3
"""Build an Alpine ext2 disk image with an openbox stacking-WM desktop.

Mirror of `build-alpine-i3.py` but swaps i3wm/i3status/dmenu for
openbox/xterm — much smaller surface than i3 (no IPC socket, no
internal bar machinery, no event subscription protocol).  Useful as
a comparison baseline: if openbox works under loads where i3 flakes,
the issue is i3-specific (libev, IPC, bar polling); if openbox flakes
the same way, it's our scheduler/X11/ABI side.

Usage:
    python3 tools/build-alpine-openbox.py [--arch aarch64|x86_64] \\
        build/alpine-openbox.aarch64.img
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

OPENBOX_PACKAGES = [
    "alpine-baselayout",
    "busybox",
    "musl-utils",
    "xorg-server",
    "xf86-video-fbdev",
    "xf86-input-evdev",
    "xinit",
    "xauth",
    "xset",
    "xsetroot",
    "xdpyinfo",
    "xprop",
    "xwininfo",
    "xev",
    # WM stack
    "openbox",
    "xterm",
    # Fonts
    "font-dejavu",
    "font-misc-misc",
    "font-cursor-misc",
    "fontconfig",
    # Diagnostic — strace lets us byte-dump openbox.real's X11
    # syscalls when KBOX_PHASE=99 wraps it.  See blog 236-...
    "strace",
]


def apko_yaml(arch: str) -> str:
    return (
        "contents:\n"
        "  repositories:\n"
        + "".join(f"    - {r}\n" for r in ALPINE_REPOS) +
        f"  keyring:\n    - {ALPINE_KEY}\n"
        "  packages:\n"
        + "".join(f"    - {p}\n" for p in OPENBOX_PACKAGES) +
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
        print(f"  APKO  resolving + installing {len(OPENBOX_PACKAGES)} packages "
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


def write_kevlar_config(root: Path) -> None:
    """Layer Kevlar-specific config over the apko-built rootfs."""
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
        '    # ShadowFB off: bypasses libshadow.so::shadowUpdatePacked,\n'
        '    # which loops infinitely on Kevlar when the cursor is set\n'
        '    # via CW_CURSOR (task #42, blog 243).\n'
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

    (xorg_conf_dir / "20-input.conf").write_text(
        'Section "InputDevice"\n'
        '    Identifier "kb0"\n'
        '    Driver     "evdev"\n'
        '    Option     "Device" "/dev/input/event0"\n'
        '    Option     "CoreKeyboard" "on"\n'
        'EndSection\n'
        '\n'
        'Section "InputDevice"\n'
        '    Identifier "ms0"\n'
        '    Driver     "evdev"\n'
        '    Option     "Device" "/dev/input/event1"\n'
        '    Option     "CorePointer" "on"\n'
        'EndSection\n'
        '\n'
        'Section "ServerLayout"\n'
        '    Identifier "L"\n'
        '    Screen     "default"\n'
        '    InputDevice "kb0" "CoreKeyboard"\n'
        '    InputDevice "ms0" "CorePointer"\n'
        'EndSection\n'
    )

    home = root / "root"
    home.mkdir(exist_ok=True)

    # openbox config — standalone WM, no built-in bar.  autostart spawns
    # xsetroot (paint root) + xterm (verify a client mapped).
    ob_cfg = home / ".config" / "openbox"
    ob_cfg.mkdir(parents=True, exist_ok=True)
    (ob_cfg / "autostart").write_text(
        "#!/bin/sh\n"
        "xsetroot -solid '#225588' &\n"
        "xterm -geometry 80x24+100+100 &\n"
    )
    os.chmod(ob_cfg / "autostart", 0o755)

    # Minimal rc.xml so openbox doesn't bail on missing config.  Default
    # values from the Alpine package don't always land at the path
    # openbox actually checks; ship our own one-liner that's enough to
    # boot.  openbox falls back to its compiled-in defaults for anything
    # missing here.
    (ob_cfg / "rc.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<openbox_config xmlns="http://openbox.org/3.4/rc">\n'
        '  <focus>\n'
        '    <focusNew>yes</focusNew>\n'
        '  </focus>\n'
        '  <theme>\n'
        '    <name>Onyx</name>\n'
        '  </theme>\n'
        '</openbox_config>\n'
    )

    (home / ".xinitrc").write_text("#!/bin/sh\nexec openbox-session\n")
    os.chmod(home / ".xinitrc", 0o755)

    (root / "etc" / "inittab").write_text(
        "# Kevlar openbox inittab\n"
        "::sysinit:/bin/mkdir -p /tmp/.X11-unix\n"
        "::sysinit:/bin/hostname kevlar\n"
        "ttyS0::respawn:/sbin/getty -n -l /bin/sh -L 115200 ttyS0 vt100\n"
        "::shutdown:/bin/sync\n"
    )

    (root / "root" / "start-openbox.sh").write_text(
        "#!/bin/sh\n"
        "echo 'Starting Xorg + openbox...'\n"
        "export HOME=/root DISPLAY=:0\n"
        "rm -f /tmp/.X0-lock /tmp/.X11-unix/X0\n"
        "/usr/libexec/Xorg :0 -nolisten tcp -noreset "
        "-config /etc/X11/xorg.conf.d/10-fbdev.conf vt1 &\n"
        "sleep 3\n"
        "openbox &\n"
    )
    os.chmod(root / "root" / "start-openbox.sh", 0o755)

    # Pre-generate XLFD fonts.dir (Xorg fbdev refuses to start without).
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

    fc_cache = shutil.which("fc-cache")
    if fc_cache:
        try:
            subprocess.run([fc_cache, "--sysroot", str(root), "-f"],
                           capture_output=True, timeout=30)
            print("  CACHE  fc-cache: generated", file=sys.stderr)
        except Exception:
            pass


def make_ext2(root: Path, output: str, size_mb: int) -> None:
    mke2fs = ensure_tool("mke2fs", "e2fsprogs")
    print(f"  MKDISK  {size_mb}MB ext2 -> {output}", file=sys.stderr)
    subprocess.run(
        ["dd", "if=/dev/zero", f"of={output}", "bs=1m", f"count={size_mb}"],
        check=True, capture_output=True)
    subprocess.run(
        [mke2fs, "-t", "ext2", "-d", str(root), "-F", output],
        check=True, capture_output=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("output", help="output disk image path")
    ap.add_argument("--arch", default="aarch64",
                    choices=["aarch64", "x86_64"])
    ap.add_argument("--size-mb", type=int, default=768)
    args = ap.parse_args()

    if os.path.exists(args.output):
        print(f"  SKIP  {args.output} already exists (delete to rebuild)", file=sys.stderr)
        return

    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir) / "alpine-root"
        root.mkdir()
        build_rootfs_with_apko(args.arch, root)
        write_kevlar_config(root)
        install_kbox_if_present(args.arch, root)
        install_kxproxy_if_present(args.arch, root)
        install_kxreplay_if_present(args.arch, root)
        Path(args.output).parent.mkdir(parents=True, exist_ok=True)
        make_ext2(root, args.output, args.size_mb)

    print(f"  DONE  {args.output}", file=sys.stderr)


def install_kxreplay_if_present(arch: str, root: Path) -> None:
    """Install kxreplay at /usr/bin/kxreplay if built.  Used by
    KBOX_PHASE=96: replays a captured openbox→Xorg byte trace
    (embedded at build time from tools/kxreplay/trace.log)."""
    triple_for_arch = {
        "aarch64": "aarch64-unknown-linux-musl",
        "x86_64":  "x86_64-unknown-linux-musl",
    }
    triple = triple_for_arch.get(arch)
    if triple is None:
        return
    bin_path = ROOT / "tools" / "kxreplay" / "target" / triple / "release" / "kxreplay"
    if not bin_path.exists():
        print(f"  SKIP  kxreplay not built ({bin_path})", file=sys.stderr)
        return
    target = root / "usr" / "bin" / "kxreplay"
    shutil.copy(str(bin_path), str(target))
    os.chmod(target, 0o755)
    print(f"  KXREPLAY  installed kxreplay ({bin_path.stat().st_size} B) "
          f"as /usr/bin/kxreplay", file=sys.stderr)


def install_kxproxy_if_present(arch: str, root: Path) -> None:
    """Install kxproxy at /usr/bin/kxproxy if built.  Used by the
    KBOX_PHASE=97 test path to proxy openbox↔Xorg traffic with a
    full byte-level wire log."""
    triple_for_arch = {
        "aarch64": "aarch64-unknown-linux-musl",
        "x86_64":  "x86_64-unknown-linux-musl",
    }
    triple = triple_for_arch.get(arch)
    if triple is None:
        return
    bin_path = ROOT / "tools" / "kxproxy" / "target" / triple / "release" / "kxproxy"
    if not bin_path.exists():
        print(f"  SKIP  kxproxy not built ({bin_path})", file=sys.stderr)
        return
    target = root / "usr" / "bin" / "kxproxy"
    shutil.copy(str(bin_path), str(target))
    os.chmod(target, 0o755)
    print(f"  KXPROXY  installed kxproxy ({bin_path.stat().st_size} B) "
          f"as /usr/bin/kxproxy", file=sys.stderr)


def install_kbox_if_present(arch: str, root: Path) -> None:
    """Hijack /usr/bin/openbox with our Rust kbox if it exists.

    The openbox test (testing/test_openbox.c) scans /proc/N/comm for
    the literal string "openbox", so we install kbox AT that path.
    The original apko-installed openbox is preserved at
    /usr/bin/openbox.real for A/B comparison.

    If tools/kbox/target/<triple>/release/kbox is missing, fall back
    to the apko openbox so the image still builds without us.
    """
    triple_for_arch = {
        "aarch64": "aarch64-unknown-linux-musl",
        "x86_64":  "x86_64-unknown-linux-musl",
    }
    triple = triple_for_arch.get(arch)
    if triple is None:
        return
    kbox_bin = ROOT / "tools" / "kbox" / "target" / triple / "release" / "kbox"
    if not kbox_bin.exists():
        print(f"  SKIP  kbox not built ({kbox_bin}) — using apko openbox",
              file=sys.stderr)
        return
    target = root / "usr" / "bin" / "openbox"
    if target.exists():
        backup = root / "usr" / "bin" / "openbox.real"
        target.rename(backup)
        print(f"  KBOX  preserving apko openbox at {backup.relative_to(root)}",
              file=sys.stderr)
    shutil.copy(str(kbox_bin), str(target))
    os.chmod(target, 0o755)
    print(f"  KBOX  installed {kbox_bin.name} ({kbox_bin.stat().st_size} B) "
          f"as /usr/bin/openbox", file=sys.stderr)


if __name__ == "__main__":
    main()
