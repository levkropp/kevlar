#!/usr/bin/env python3
"""xfce-screenshot — boot Kevlar+Alpine-XFCE and capture framebuffer screenshots.

Boots with QEMU's QMP socket exposed, waits for XFCE to come up, then issues
`screendump` commands at intervals to capture PNG screenshots into
build/xfce-shots/.

Usage:
    tools/xfce-screenshot.py --boot-secs 60 --shots 5 --interval 10
"""

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
QMP_SOCK = "/tmp/kevlar-xfce-qmp.sock"
OUT_DIR = REPO / "build" / "xfce-shots"


def qmp_send(sock, cmd, **args):
    msg = {"execute": cmd}
    if args:
        msg["arguments"] = args
    sock.sendall((json.dumps(msg) + "\n").encode())


def qmp_recv(sock, timeout=5.0):
    sock.settimeout(timeout)
    buf = b""
    while True:
        chunk = sock.recv(4096)
        if not chunk:
            return None
        buf += chunk
        # QMP messages are newline-delimited JSON.
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            try:
                msg = json.loads(line.decode())
                if "return" in msg or "error" in msg:
                    return msg
            except json.JSONDecodeError:
                continue


def qmp_handshake(sock):
    """Read the initial QMP greeting and send qmp_capabilities."""
    sock.settimeout(5.0)
    greeting = sock.recv(4096)
    if b"QMP" not in greeting:
        raise RuntimeError(f"unexpected QMP greeting: {greeting!r}")
    qmp_send(sock, "qmp_capabilities")
    qmp_recv(sock)


def screendump(sock, path):
    """Capture a screenshot to `path` (PPM format)."""
    qmp_send(sock, "screendump", filename=str(path))
    resp = qmp_recv(sock, timeout=30.0)
    return resp


def ppm_to_png(ppm_path, png_path):
    subprocess.run(["magick", str(ppm_path), str(png_path)], check=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--boot-secs", type=int, default=45,
                    help="Seconds to wait after launch before first screenshot (default: 45)")
    ap.add_argument("--shots", type=int, default=3,
                    help="Number of screenshots to capture (default: 3)")
    ap.add_argument("--interval", type=int, default=10,
                    help="Seconds between screenshots (default: 10)")
    ap.add_argument("--profile", default="balanced")
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--init", default="/bin/test-xfce-idle",
                    help="INIT_SCRIPT path (default: /bin/test-xfce-idle)")
    ap.add_argument("--keep-running", action="store_true",
                    help="Don't kill QEMU after capture")
    args = ap.parse_args()

    # Ensure the alpine-xfce image exists.
    img_path = REPO / "build" / "alpine-xfce.img"
    if not img_path.exists():
        print(f"[xfce-shot] {img_path} missing — run `make build/alpine-xfce.img` first.",
              file=sys.stderr)
        return 2

    # Build the kernel. Default init is /bin/test-xfce-idle (full XFCE then
    # idle); alternative is /bin/test-x11-visible (minimal X11 + xterm).
    print(f"[xfce-shot] building kernel (INIT_SCRIPT={args.init})...", file=sys.stderr)
    subprocess.run(["make", "build", f"PROFILE={args.profile}",
                    f"INIT_SCRIPT={args.init}"],
                   cwd=REPO, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for f in OUT_DIR.glob("*.ppm"):
        f.unlink()
    for f in OUT_DIR.glob("*.png"):
        f.unlink()

    # Clean stale QMP sock.
    try:
        os.unlink(QMP_SOCK)
    except FileNotFoundError:
        pass

    # QEMU's built-in multiboot loader needs e_machine=EM_386 (the bzImage
    # boot path through SeaBIOS's linuxboot.rom is unreliable in QEMU 10.x).
    # Same patch run-qemu.py applies — we replicate it here so we control
    # the full QEMU invocation (display, QMP socket).
    import tempfile
    src_elf = REPO / "kevlar.x64.elf"
    if not src_elf.exists():
        print(f"[xfce-shot] {src_elf} missing — `make build` must succeed first.", file=sys.stderr)
        return 2
    with open(src_elf, "rb") as f:
        elf_data = bytearray(f.read())
    elf_data[18] = 0x03  # EM_386 low byte
    elf_data[19] = 0x00
    tmp_fd, tmp_elf = tempfile.mkstemp(suffix=".elf")
    os.write(tmp_fd, elf_data)
    os.close(tmp_fd)

    qemu_cmd = [
        "qemu-system-x86_64",
        "-cpu", "Icelake-Server",
        "-m", "1024",
        "-smp", str(args.smp),
        "-accel", "kvm",
        "-no-reboot",
        "-mem-prealloc",
        "-vga", "std",
        "-display", "none",
        "-serial", "file:/tmp/kevlar-xfce-shot.serial",
        "-monitor", "none",
        "-qmp", f"unix:{QMP_SOCK},server=on,wait=off",
        "-kernel", tmp_elf,
        "-drive", f"file={img_path},format=raw,if=virtio",
        "-device", "isa-debug-exit,iobase=0x501,iosize=2",
        "-device", "virtio-net,netdev=net0,disable-legacy=on,disable-modern=off",
        "-netdev", "user,id=net0",
    ]

    print(f"[xfce-shot] launching QEMU (display=none, QMP={QMP_SOCK})", file=sys.stderr)
    qemu = subprocess.Popen(qemu_cmd, cwd=REPO,
                             stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)

    # Wait for QMP socket to appear.
    deadline = time.time() + 10
    while not os.path.exists(QMP_SOCK):
        if time.time() > deadline:
            qemu.kill()
            print("[xfce-shot] QMP socket never appeared", file=sys.stderr)
            return 1
        time.sleep(0.1)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(QMP_SOCK)
    qmp_handshake(sock)
    print("[xfce-shot] QMP connected", file=sys.stderr)

    print(f"[xfce-shot] sleeping {args.boot_secs}s for XFCE to come up...", file=sys.stderr)
    time.sleep(args.boot_secs)

    for i in range(args.shots):
        ppm = OUT_DIR / f"shot-{i:02d}.ppm"
        png = OUT_DIR / f"shot-{i:02d}.png"
        print(f"[xfce-shot] shot {i+1}/{args.shots} -> {ppm.name}", file=sys.stderr)
        resp = screendump(sock, ppm)
        if resp and "error" in resp:
            print(f"[xfce-shot] screendump error: {resp['error']}", file=sys.stderr)
        elif ppm.exists():
            try:
                ppm_to_png(ppm, png)
                size = png.stat().st_size
                print(f"[xfce-shot]   -> {png.name} ({size} bytes)", file=sys.stderr)
            except Exception as e:
                print(f"[xfce-shot]   PPM->PNG failed: {e}", file=sys.stderr)
        if i < args.shots - 1:
            time.sleep(args.interval)

    sock.close()

    if args.keep_running:
        print("[xfce-shot] --keep-running set; QEMU pid=" + str(qemu.pid), file=sys.stderr)
        return 0

    print("[xfce-shot] killing QEMU", file=sys.stderr)
    qemu.terminate()
    try:
        qemu.wait(timeout=5)
    except subprocess.TimeoutExpired:
        qemu.kill()

    print(f"[xfce-shot] done. screenshots in {OUT_DIR}/", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
