#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 5 host smoke test.  Exercises the pixmap path:
#   1. Create a window and map it.
#   2. Create an off-screen pixmap.
#   3. PutImage a 4x4 red/blue checkerboard into the pixmap.
#   4. CopyArea the pixmap onto the window.
#   5. GetImage the same region back from the window and check parity.
#   6. On exit, verify the dumped PPM shows the pattern at screen coords.

import os
import signal
import socket
import struct
import subprocess
import sys
import time

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "target",
    "x86_64-unknown-linux-musl",
    "release",
    "kxserver",
)
DISPLAY = 98
PPM_PATH = "/tmp/kxserver-phase5.ppm"
ABSTRACT_PATH = f"/tmp/.X11-unix/X{DISPLAY}"
ROOT_WID = 0x20

RED  = 0x00FF0000
BLUE = 0x000000FF

def connect_abstract():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    for _ in range(50):
        try:
            s.connect("\0" + ABSTRACT_PATH)
            return s
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.05)
    raise RuntimeError("could not connect")

def recv_all(s, n):
    out = b""
    while len(out) < n:
        chunk = s.recv(n - len(out))
        if not chunk:
            raise RuntimeError("short recv")
        out += chunk
    return out

def setup_handshake(s):
    s.sendall(struct.pack("<BxHHHHxx", 0x6C, 11, 0, 0, 0))
    head = recv_all(s, 8)
    assert head[0] == 1, f"setup failed: {head!r}"
    extra_words = struct.unpack_from("<H", head, 6)[0]
    body = recv_all(s, extra_words * 4)
    rid_base = struct.unpack_from("<I", body, 4)[0]
    return rid_base

def send_create_window(s, wid, parent, x, y, w, h):
    body = struct.pack("<IIhhHHHHII",
        wid, parent, x, y, w, h, 0, 1, 0, 0)
    length = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 1, 0, length) + body)

def send_map_window(s, wid):
    s.sendall(struct.pack("<BBHI", 8, 0, 2, wid))

def send_create_pixmap(s, pid, parent, w, h, depth=24):
    body = struct.pack("<IIHH", pid, parent, w, h)
    length = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 53, depth, length) + body)

def send_put_image_zpixmap(s, did, gid, w, h, dx, dy, pixels, depth=24):
    body = struct.pack("<IIHHHHBB2x",
        did, gid, w, h, dx, dy, 0, depth)
    # Pack pixels as little-endian u32s.
    data = b"".join(struct.pack("<I", p) for p in pixels)
    # X11 pads to 4 bytes.
    pad = (-len(data)) & 3
    data = data + b"\0" * pad
    header_body = body + data
    length = (4 + len(header_body)) // 4
    s.sendall(struct.pack("<BBH", 72, 2, length) + header_body)  # format=2 (ZPixmap)

def send_copy_area(s, src, dst, gid, sx, sy, dx, dy, w, h):
    body = struct.pack("<IIIhhhhHH",
        src, dst, gid, sx, sy, dx, dy, w, h)
    length = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 62, 0, length) + body)

def send_create_gc(s, gid, drawable, fg_pixel=0):
    mask = 0x04
    body = struct.pack("<IIII", gid, drawable, mask, fg_pixel)
    length = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 55, 0, length) + body)

def send_get_image(s, did, x, y, w, h):
    body = struct.pack("<IhhHHI", did, x, y, w, h, 0xFFFFFFFF)
    length = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 73, 2, length) + body)  # format=2 (ZPixmap)

def read_ppm(path):
    with open(path, "rb") as f:
        data = f.read()
    assert data.startswith(b"P6\n")
    nl1 = data.index(b"\n")
    nl2 = data.index(b"\n", nl1 + 1)
    nl3 = data.index(b"\n", nl2 + 1)
    w, h = map(int, data[nl1+1:nl2].split())
    return w, h, data[nl3+1:]

def pixel(pixels, w, x, y):
    off = (y * w + x) * 3
    return pixels[off], pixels[off+1], pixels[off+2]

def main():
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
        s = connect_abstract()
        rid_base = setup_handshake(s)
        print(f"rid_base={rid_base:#x}")

        wid = rid_base | 1
        pid = rid_base | 2
        gid = rid_base | 3

        # Window at (200, 100) with size 100x100, pixmap 4x4 checker inside.
        send_create_window(s, wid, ROOT_WID, 200, 100, 100, 100)
        send_map_window(s, wid)
        send_create_pixmap(s, pid, wid, 4, 4, depth=24)
        send_create_gc(s, gid, wid, fg_pixel=0)

        # 4x4 checker: red on even cells, blue on odd cells.
        checker = []
        for y in range(4):
            for x in range(4):
                checker.append(RED if (x + y) % 2 == 0 else BLUE)

        send_put_image_zpixmap(s, pid, gid, 4, 4, 0, 0, checker)
        # Copy the pixmap to the window at offset (10, 10).
        send_copy_area(s, pid, wid, gid, 0, 0, 10, 10, 4, 4)

        # Read pixels back from the window at (10, 10) to verify the blit.
        send_get_image(s, wid, 10, 10, 4, 4)
        # Reply: 32-byte header + 4*4*4 = 64 bytes of pixel data = 96 bytes
        head = recv_all(s, 32)
        assert head[0] == 1, f"GetImage reply not a reply: {head[:4]!r}"
        length_words = struct.unpack_from("<I", head, 4)[0]
        body = recv_all(s, length_words * 4)
        got_pixels = [struct.unpack_from("<I", body, i*4)[0] for i in range(16)]
        print(f"GetImage returned {len(got_pixels)} pixels:")
        for y in range(4):
            row = " ".join(f"{got_pixels[y*4+x]:08x}" for x in range(4))
            print("  ", row)

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

    # Verify GetImage returned the checker pattern.
    expected = []
    for y in range(4):
        for x in range(4):
            expected.append(RED if (x + y) % 2 == 0 else BLUE)
    if got_pixels != expected:
        print("FAIL: GetImage pixels did not match expected checker")
        print(" got:", [f"{p:08x}" for p in got_pixels])
        print(" exp:", [f"{p:08x}" for p in expected])
        return 1

    # Verify PPM shows the checker at screen (210+k, 110+k).
    w, h, pixels = read_ppm(PPM_PATH)
    # Cell (0,0): red at screen (210, 110)
    p00 = pixel(pixels, w, 210, 110)
    # Cell (1,0): blue at screen (211, 110)
    p10 = pixel(pixels, w, 211, 110)
    # Cell (1,1): red at screen (211, 111)
    p11 = pixel(pixels, w, 211, 111)
    print(f"cells: (0,0)={p00} (1,0)={p10} (1,1)={p11}")
    if p00 != (0xFF, 0, 0):
        print(f"FAIL: (0,0) expected red, got {p00}")
        return 1
    if p10 != (0, 0, 0xFF):
        print(f"FAIL: (1,0) expected blue, got {p10}")
        return 1
    if p11 != (0xFF, 0, 0):
        print(f"FAIL: (1,1) expected red, got {p11}")
        return 1

    print("PASS: Phase 5 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
