#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 4 host smoke test.  Spawns kxserver with a shadow framebuffer,
# drives the wire protocol through CreateWindow / MapWindow / CreateGC /
# PolyFillRectangle, then sends SIGTERM and checks that the dumped PPM
# contains a red rectangle at the expected location.
#
# Run from the kxserver workspace root after `cargo build --release`.

import os
import socket
import struct
import subprocess
import sys
import time
import signal

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "target",
    "x86_64-unknown-linux-musl",
    "release",
    "kxserver",
)
DISPLAY = 99
PPM_PATH = "/tmp/kxserver-phase4.ppm"
ABSTRACT_PATH = f"/tmp/.X11-unix/X{DISPLAY}"

ROOT_WID = 0x20

# ─── wire helpers ────────────────────────────────────────────────────
def pad4(n):
    return (4 - (n & 3)) & 3

def connect_abstract():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    addr = "\0" + ABSTRACT_PATH
    for _ in range(50):
        try:
            s.connect(addr)
            return s
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.05)
    raise RuntimeError(f"could not connect to {ABSTRACT_PATH!r}")

def setup_handshake(s):
    # 12-byte setup: little-endian marker 0x6c, pad, major=11, minor=0, 0,0,0,0
    s.sendall(struct.pack("<BxHHHHxx", 0x6C, 11, 0, 0, 0))
    head = s.recv(8)
    assert len(head) == 8 and head[0] == 1, f"setup failed: {head!r}"
    extra_words = struct.unpack_from("<H", head, 6)[0]
    body = b""
    want = extra_words * 4
    while len(body) < want:
        chunk = s.recv(want - len(body))
        if not chunk:
            raise RuntimeError("short setup body")
        body += chunk
    # resource-id-base at offset 4, mask at 8 (within body).
    rid_base = struct.unpack_from("<I", body, 4)[0]
    rid_mask = struct.unpack_from("<I", body, 8)[0]
    return rid_base, rid_mask

def req(opcode, data_byte, words):
    length = 1 + len(words)  # words here excludes header
    header = struct.pack("<BBH", opcode, data_byte, length)
    return header + b"".join(words)

def send_create_window(s, wid, parent, x, y, w, h):
    body = struct.pack(
        "<IIhhHHHHII",
        wid, parent,
        x, y, w, h,
        0,          # border_width
        1,          # class = InputOutput
        0,          # visual = CopyFromParent
        0,          # value_mask
    )
    # req header: opcode=1, depth=0 (CopyFromParent), length = (4+len)/4
    length_words = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 1, 0, length_words) + body)

def send_map_window(s, wid):
    s.sendall(struct.pack("<BBHI", 8, 0, 2, wid))

def send_create_gc(s, gid, drawable, fg_pixel):
    # GCForeground = bit 2 → mask 0x00000004, one 32-bit value.
    mask = 0x04
    body = struct.pack("<IIII", gid, drawable, mask, fg_pixel)
    length_words = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 55, 0, length_words) + body)

def send_poly_fill_rect(s, wid, gid, rects):
    body = struct.pack("<II", wid, gid)
    for (x, y, w, h) in rects:
        body += struct.pack("<hhHH", x, y, w, h)
    length_words = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 70, 0, length_words) + body)

# ─── PPM checker ─────────────────────────────────────────────────────
def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    # P6\nW H\n255\n<bytes>
    assert data.startswith(b"P6\n"), "not a P6 PPM"
    # parse header
    nl1 = data.index(b"\n")
    nl2 = data.index(b"\n", nl1 + 1)
    nl3 = data.index(b"\n", nl2 + 1)
    w, h = map(int, data[nl1+1:nl2].split())
    pixels = data[nl3+1:]
    return w, h, pixels

def pixel(pixels, w, x, y):
    off = (y * w + x) * 3
    return pixels[off], pixels[off+1], pixels[off+2]

# ─── main ────────────────────────────────────────────────────────────
def main():
    if not os.path.exists(BIN):
        print(f"FAIL: kxserver binary not built at {BIN}")
        return 1

    if os.path.exists(PPM_PATH):
        os.unlink(PPM_PATH)

    proc = subprocess.Popen(
        [BIN, f":{DISPLAY}",
         f"--ppm-on-exit={PPM_PATH}",
         "--log=warn"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        # Wait for listening.
        s = connect_abstract()
        rid_base, rid_mask = setup_handshake(s)
        print(f"rid_base={rid_base:#x} mask={rid_mask:#x}")

        wid = rid_base | 1
        gid = rid_base | 2
        send_create_window(s, wid, ROOT_WID, 100, 80, 200, 150)
        send_map_window(s, wid)
        # Red in BGRA8888 host byte order: 0x00FF0000 for pack_pixel we
        # pass already-packed pixel since mask=GCForeground stores it raw.
        send_create_gc(s, gid, wid, 0x00FF0000)
        send_poly_fill_rect(s, wid, gid, [(10, 10, 180, 130)])
        s.close()
        time.sleep(0.15)
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            out, _ = proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            out, _ = proc.communicate()
        print("--- kxserver log ---")
        print(out)
        print("--- end log ---")

    if not os.path.exists(PPM_PATH):
        print("FAIL: no PPM produced")
        return 1

    w, h, pixels = read_ppm(PPM_PATH)
    if (w, h) != (1024, 768):
        print(f"FAIL: unexpected PPM size {w}x{h}")
        return 1

    # Window at (100,80) in root, fill at (10,10) inside window, 180×130.
    # Center of filled area: (100+10+90, 80+10+65) = (200, 155).
    center = pixel(pixels, w, 200, 155)
    edge_out = pixel(pixels, w, 50, 50)  # outside window (root bg default 0)
    print(f"center={center} outside={edge_out}")
    if center != (0xFF, 0x00, 0x00):
        print(f"FAIL: expected red at (200,155), got {center}")
        return 1
    # A pixel inside the window but outside the filled area should NOT be red.
    inside_unfilled = pixel(pixels, w, 102, 82)
    if inside_unfilled == (0xFF, 0x00, 0x00):
        print(f"FAIL: red bleed at (102,82): {inside_unfilled}")
        return 1

    print("PASS: Phase 4 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
