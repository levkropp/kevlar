#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 10 smoke test: RENDER extension.
#
# Verifies the Xft hot path end-to-end:
#
#   QueryExtension(RENDER)  → present=1, major=129
#   QueryVersion            → 0.11
#   QueryPictFormats        → A8 and A8R8G8B8 both present
#   CreatePicture           → drawable-backed Picture on a window
#   CreateSolidFill         → a solid-red source
#   CreateGlyphSet(A8)
#   AddGlyphs               → upload one 4×4 glyph with a checker alpha
#                             pattern [255, 0; 0, 255; ...]
#   PolyFillRectangle       → paint the window blue (dst background)
#   CompositeGlyphs8(Over)  → draw the glyph at a known position
#   GetImage                → verify pixels match the expected blend
#   FillRectangles          → red opaque rectangle, verify pixels

import os, signal, socket, struct, subprocess, sys, time

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "target", "x86_64-unknown-linux-musl", "release", "kxserver",
)
DISPLAY = 93
ABSTRACT = f"/tmp/.X11-unix/X{DISPLAY}"
PPM_PATH = "/tmp/kxserver-phase10.ppm"
ROOT_WID = 0x20

RENDER_MAJOR = 129
PICTFMT_A8R8G8B8 = 0x10
PICTFMT_A8       = 0x13

PICT_OP_SRC  = 1
PICT_OP_OVER = 3

def connect():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    for _ in range(50):
        try:
            s.connect("\0" + ABSTRACT); return s
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.05)
    raise RuntimeError("no connect")

def recv_all(s, n, timeout=2.0):
    s.settimeout(timeout)
    out = b""
    while len(out) < n:
        chunk = s.recv(n - len(out))
        if not chunk: raise RuntimeError("short recv")
        out += chunk
    s.settimeout(None)
    return out

def handshake(s):
    s.sendall(struct.pack("<BxHHHHxx", 0x6C, 11, 0, 0, 0))
    head = recv_all(s, 8)
    assert head[0] == 1
    extra = struct.unpack_from("<H", head, 6)[0]
    body = recv_all(s, extra * 4)
    return struct.unpack_from("<I", body, 4)[0]

def recv_reply(s):
    head = recv_all(s, 32)
    assert head[0] == 1, f"expected reply, got {head[:4].hex()}"
    extra = struct.unpack_from("<I", head, 4)[0]
    body = recv_all(s, extra * 4) if extra else b""
    return head + body

def query_extension(s, name):
    nb = name.encode()
    pad = (-len(nb)) & 3
    body = struct.pack("<HH", len(nb), 0) + nb + b"\0"*pad
    s.sendall(struct.pack("<BBH", 98, 0, (4+len(body))//4) + body)
    return recv_reply(s)

def create_window(s, wid, parent, x, y, w, h):
    body = struct.pack("<IIhhHHHHII",
        wid, parent, x, y, w, h, 0, 1, 0, 0)
    s.sendall(struct.pack("<BBH", 1, 0, (4+len(body))//4) + body)

def map_window(s, wid):
    s.sendall(struct.pack("<BBHI", 8, 0, 2, wid))

def create_gc(s, gid, drawable, fg):
    body = struct.pack("<IIII", gid, drawable, 0x04, fg)
    s.sendall(struct.pack("<BBH", 55, 0, (4+len(body))//4) + body)

def poly_fill_rect(s, wid, gid, x, y, w, h):
    body = struct.pack("<IIhhHH", wid, gid, x, y, w, h)
    s.sendall(struct.pack("<BBH", 70, 0, (4+len(body))//4) + body)

def get_image(s, wid, x, y, w, h):
    body = struct.pack("<IhhHHI", wid, x, y, w, h, 0xFFFFFFFF)
    s.sendall(struct.pack("<BBH", 73, 2, (4+len(body))//4) + body)
    return recv_reply(s)

# ── RENDER requests ────────────────────────────────────────────────
def render_query_version(s, major=0, minor=11):
    body = struct.pack("<II", major, minor)
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 0, (4+len(body))//4) + body)
    return recv_reply(s)

def render_query_pict_formats(s):
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 1, 1))
    return recv_reply(s)

def render_create_picture(s, pid, drawable, fmt):
    body = struct.pack("<IIII", pid, drawable, fmt, 0)  # mask=0 → no values
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 4, (4+len(body))//4) + body)

def render_free_picture(s, pid):
    body = struct.pack("<I", pid)
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 7, (4+len(body))//4) + body)

def render_create_solid_fill(s, pid, r, g, b, a):
    body = struct.pack("<IHHHH", pid, r, g, b, a)
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 33, (4+len(body))//4) + body)

def render_create_glyph_set(s, gsid, fmt):
    body = struct.pack("<II", gsid, fmt)
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 17, (4+len(body))//4) + body)

def render_add_glyphs_a8(s, gsid, glyph_id, width, height, x, y, x_off, y_off, alphas):
    # alphas is an iterable of u8 row-major (width*height entries).
    assert len(alphas) == width * height
    body = struct.pack("<II", gsid, 1)  # 1 glyph
    body += struct.pack("<I", glyph_id)
    body += struct.pack("<HHhhhh", width, height, x, y, x_off, y_off)
    # Image data: each row padded to 4 bytes.
    row_pad = (-width) & 3
    for row in range(height):
        body += bytes(alphas[row*width:(row+1)*width])
        body += b"\0" * row_pad
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 20, (4+len(body))//4) + body)

def render_composite_glyphs_8(s, op, src_pid, dst_pid, gsid, glyph_x, glyph_y, glyph_ids):
    # Real RENDER CompositeGlyphs8 wire layout:
    #   byte 4: op (Porter-Duff)
    #   bytes 5..8: pad
    #   8..12 src, 12..16 dst, 16..20 mask_format,
    #   20..24 gsid, 24..26 glyph_x, 26..28 glyph_y
    #   28+: GLYPHELT8 list
    body = struct.pack("<B3xIIIIhh", op, src_pid, dst_pid, 0, gsid, glyph_x, glyph_y)
    n = len(glyph_ids)
    elt = bytes([n, 0, 0, 0]) + struct.pack("<hh", 0, 0) + bytes(glyph_ids)
    while len(elt) % 4 != 0: elt += b"\0"
    body += elt
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 23, (4+len(body))//4) + body)

def render_fill_rectangles(s, op, dst_pid, r, g, b, a, rects):
    # Real RENDER FillRectangles wire layout:
    #   byte 4: op
    #   bytes 5..8: pad
    #   8..12 dst, 12..14 r, 14..16 g, 16..18 b, 18..20 a
    #   20+: rects
    body = struct.pack("<B3xIHHHH", op, dst_pid, r, g, b, a)
    for (x, y, w, h) in rects:
        body += struct.pack("<hhHH", x, y, w, h)
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 26, (4+len(body))//4) + body)

def main():
    if os.path.exists(PPM_PATH):
        os.unlink(PPM_PATH)
    proc = subprocess.Popen(
        [BIN, f":{DISPLAY}", f"--ppm-on-exit={PPM_PATH}", "--log=warn"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    failures = []
    try:
        s = connect()
        base = handshake(s)
        print(f"base={base:#x}")

        # (1) Extension present.
        r = query_extension(s, "RENDER")
        print(f"RENDER: present={r[8]} major={r[9]}")
        if r[8] != 1 or r[9] != RENDER_MAJOR:
            failures.append(f"RENDER missing: present={r[8]} major={r[9]}")

        # (2) QueryVersion.
        r = render_query_version(s)
        rv_major = struct.unpack_from("<I", r, 8)[0]
        rv_minor = struct.unpack_from("<I", r, 12)[0]
        print(f"RENDER QueryVersion → {rv_major}.{rv_minor}")
        if (rv_major, rv_minor) != (0, 11):
            failures.append(f"QueryVersion {rv_major}.{rv_minor} != 0.11")

        # (3) QueryPictFormats — confirm A8 and A8R8G8B8 appear.
        r = render_query_pict_formats(s)
        num_formats = struct.unpack_from("<I", r, 8)[0]
        print(f"QueryPictFormats: num_formats={num_formats}")
        # Body starts at 32 + 0 (we already stuffed the header into r).
        # Each format is 28 bytes; the first one starts at offset 32.
        format_ids = []
        for i in range(num_formats):
            off = 32 + i * 28
            fid = struct.unpack_from("<I", r, off)[0]
            format_ids.append(fid)
        print(f"  format ids: {[hex(f) for f in format_ids]}")
        if PICTFMT_A8 not in format_ids:
            failures.append("A8 not advertised")
        if PICTFMT_A8R8G8B8 not in format_ids:
            failures.append("A8R8G8B8 not advertised")

        # (4) Create the test window + a GC to paint its background.
        wid = base | 1
        gid = base | 2
        dst_pic = base | 3
        src_pic = base | 4
        gset    = base | 5
        create_window(s, wid, ROOT_WID, 100, 100, 200, 200)
        map_window(s, wid)
        create_gc(s, gid, wid, 0x000000FF)   # blue core fg
        poly_fill_rect(s, wid, gid, 0, 0, 200, 200)  # background = blue

        # (5) CreatePicture on the window.
        render_create_picture(s, dst_pic, wid, PICTFMT_A8R8G8B8)
        # (6) CreateSolidFill: opaque RED.
        render_create_solid_fill(s, src_pic, 0xFFFF, 0x0000, 0x0000, 0xFFFF)
        # (7) CreateGlyphSet(A8)
        render_create_glyph_set(s, gset, PICTFMT_A8)

        # (8) AddGlyphs: a 4x4 checker with alpha 255 on even cells, 0 on odd.
        alphas = []
        for gy in range(4):
            for gx in range(4):
                alphas.append(0xFF if (gx + gy) % 2 == 0 else 0x00)
        render_add_glyphs_a8(
            s, gset, glyph_id=0x41, width=4, height=4,
            x=0, y=0, x_off=4, y_off=0, alphas=alphas,
        )

        # (9) CompositeGlyphs8 at glyph origin (50, 50) within the window.
        render_composite_glyphs_8(
            s, PICT_OP_OVER, src_pic, dst_pic, gset,
            glyph_x=50, glyph_y=50, glyph_ids=[0x41],
        )

        # (10) GetImage: 6x6 region starting at window-local (48,48) covering
        # the 4x4 glyph area.  Glyph is drawn at (50,50)..(54,54) in window
        # space.
        r = get_image(s, wid, 48, 48, 6, 6)
        body = r[32:]
        pixels = []
        for i in range(6*6):
            pixels.append(struct.unpack_from("<I", body, i*4)[0])
        print("GetImage 6x6 around glyph:")
        for row in range(6):
            cells = [f"{pixels[row*6+col] & 0xFFFFFF:06x}" for col in range(6)]
            print("  " + " ".join(cells))
        # Glyph at (50..54, 50..54) in window coords; getimage starts at
        # (48,48).  So checker cells fall at pixel indices (2..6, 2..6)
        # within the 6x6 block.  Cell (gx=0,gy=0) = on → red at window
        # (50,50) = getimage (2,2).
        def p(x, y): return pixels[y*6 + x] & 0xFFFFFF
        # Top-left of glyph: red (alpha=255)
        if p(2,2) != 0xFF0000:
            failures.append(f"(2,2) expected red, got {p(2,2):06x}")
        # (2,3) is checker (gx=1, gy=0) → blue (background)
        if p(3,2) != 0x0000FF:
            failures.append(f"(3,2) expected blue, got {p(3,2):06x}")
        # (2,4) is (gx=2, gy=0) → red again
        if p(4,2) != 0xFF0000:
            failures.append(f"(4,2) expected red, got {p(4,2):06x}")
        # (2,5) is (gx=3, gy=0) → blue
        if p(5,2) != 0x0000FF:
            failures.append(f"(5,2) expected blue, got {p(5,2):06x}")
        # Row below: (gx=0, gy=1) = blue, (gx=1, gy=1) = red
        if p(2,3) != 0x0000FF:
            failures.append(f"(2,3) expected blue, got {p(2,3):06x}")
        if p(3,3) != 0xFF0000:
            failures.append(f"(3,3) expected red, got {p(3,3):06x}")
        # Area outside the glyph at (0,0) and (0,5) should be blue bg.
        if p(0,0) != 0x0000FF:
            failures.append(f"(0,0) expected blue, got {p(0,0):06x}")

        # (11) FillRectangles Over: draw a 10x10 green rect at window (80,80).
        render_fill_rectangles(
            s, PICT_OP_OVER, dst_pic,
            r=0x0000, g=0xFFFF, b=0x0000, a=0xFFFF,
            rects=[(80, 80, 10, 10)],
        )
        # Read back a pixel inside the fill.
        r = get_image(s, wid, 85, 85, 1, 1)
        body = r[32:]
        p85 = struct.unpack_from("<I", body, 0)[0] & 0xFFFFFF
        print(f"FillRectangles (85,85) → {p85:06x}")
        if p85 != 0x00FF00:
            failures.append(f"Fill (85,85) expected green, got {p85:06x}")

        # (12) FreePicture cleanup.
        render_free_picture(s, dst_pic)
        render_free_picture(s, src_pic)

        s.close()
        time.sleep(0.15)
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            out, _ = proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            out, _ = proc.communicate()
        print("--- server log ---")
        print(out)
        print("--- end log ---")

    if failures:
        print("FAIL:")
        for f in failures:
            print(" ", f)
        return 1
    print("PASS: Phase 10 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
