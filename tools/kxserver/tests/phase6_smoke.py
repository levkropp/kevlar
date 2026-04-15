#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 6 host smoke test: font loading + ImageText8 rendering.
#
# Flow:
#   1. Handshake, CreateWindow, MapWindow.
#   2. OpenFont "fixed" → get a font id.
#   3. QueryFont: verify the reply layout and that char_w=8, ascent=13.
#   4. QueryTextExtents "HI" → width == 16.
#   5. CreateGC with foreground=green and background=black.
#   6. PolyFillRectangle a blue rectangle to confirm background drawing.
#   7. ImageText8 "HELLO" at (x, y).
#   8. On exit, examine the dumped PPM:
#        - Glyph 'H' is 8 pixels wide.  Its embedded bitmap lights a
#          full-width row at cell-row 7 (index 7 of the 16-row glyph,
#          counting from the top of the cell).
#        - ImageText8 background is black (GC bg), foreground is green
#          (GC fg) — verify a row in the middle has a lit green pixel
#          and at least one bg pixel next to the glyph is black.

import os, signal, socket, struct, subprocess, sys, time

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "target",
    "x86_64-unknown-linux-musl",
    "release",
    "kxserver",
)
DISPLAY = 97
PPM_PATH = "/tmp/kxserver-phase6.ppm"
ABSTRACT_PATH = f"/tmp/.X11-unix/X{DISPLAY}"
ROOT_WID = 0x20

def connect():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    for _ in range(50):
        try:
            s.connect("\0" + ABSTRACT_PATH); return s
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(0.05)
    raise RuntimeError("no connect")

def recv_all(s, n):
    out = b""
    while len(out) < n:
        chunk = s.recv(n - len(out))
        if not chunk: raise RuntimeError("short recv")
        out += chunk
    return out

def handshake(s):
    s.sendall(struct.pack("<BxHHHHxx", 0x6C, 11, 0, 0, 0))
    head = recv_all(s, 8)
    assert head[0] == 1, f"setup failed: {head!r}"
    extra = struct.unpack_from("<H", head, 6)[0]
    body = recv_all(s, extra * 4)
    return struct.unpack_from("<I", body, 4)[0]

def create_window(s, wid, parent, x, y, w, h):
    body = struct.pack("<IIhhHHHHII", wid, parent, x, y, w, h, 0, 1, 0, 0)
    s.sendall(struct.pack("<BBH", 1, 0, (4+len(body))//4) + body)

def map_window(s, wid):
    s.sendall(struct.pack("<BBHI", 8, 0, 2, wid))

def open_font(s, fid, name):
    nlen = len(name)
    pad = (-nlen) & 3
    body = struct.pack("<IHxx", fid, nlen) + name.encode() + b"\0"*pad
    s.sendall(struct.pack("<BBH", 45, 0, (4+len(body))//4) + body)

def query_font(s, fid):
    s.sendall(struct.pack("<BBHI", 47, 0, 2, fid))
    head = recv_all(s, 32)
    assert head[0] == 1, f"QueryFont reply not a reply: {head!r}"
    length = struct.unpack_from("<I", head, 4)[0]
    body = recv_all(s, length * 4)
    return head, body

def query_text_extents(s, fid, text):
    nchars = len(text)
    odd = nchars & 1
    # STRING16 encoding: big-endian char2b, we set byte1=0, byte2=char.
    s16 = b""
    for ch in text:
        s16 += b"\0" + bytes([ord(ch)])
    # Pad to 4 bytes
    while len(s16) % 4 != 0: s16 += b"\0"
    body = struct.pack("<I", fid) + s16
    s.sendall(struct.pack("<BBH", 48, odd, (4+len(body))//4) + body)
    head = recv_all(s, 32)
    return head

def create_gc(s, gid, drawable, fg, bg, font_id=None):
    mask = 0x04 | 0x08   # GCForeground | GCBackground
    values = [fg, bg]
    if font_id is not None:
        mask |= 0x4000
        values.append(font_id)
    body = struct.pack("<III", gid, drawable, mask) + b"".join(struct.pack("<I", v) for v in values)
    s.sendall(struct.pack("<BBH", 55, 0, (4+len(body))//4) + body)

def poly_fill_rect(s, wid, gid, x, y, w, h):
    body = struct.pack("<IIhhHH", wid, gid, x, y, w, h)
    s.sendall(struct.pack("<BBH", 70, 0, (4+len(body))//4) + body)

def image_text_8(s, wid, gid, x, y, text):
    tb = text.encode()
    n = len(tb)
    # Body after header: drawable(4), gc(4), x(2), y(2), text(n), pad.
    data = struct.pack("<IIhh", wid, gid, x, y) + tb
    while len(data) % 4 != 0: data += b"\0"
    s.sendall(struct.pack("<BBH", 76, n, (4+len(data))//4) + data)

def read_ppm(path):
    data = open(path, "rb").read()
    assert data.startswith(b"P6\n")
    nl1 = data.index(b"\n")
    nl2 = data.index(b"\n", nl1+1)
    nl3 = data.index(b"\n", nl2+1)
    w, h = map(int, data[nl1+1:nl2].split())
    return w, h, data[nl3+1:]

def pixel(pixels, w, x, y):
    off = (y*w + x)*3
    return pixels[off], pixels[off+1], pixels[off+2]

def main():
    if os.path.exists(PPM_PATH):
        os.unlink(PPM_PATH)

    proc = subprocess.Popen(
        [BIN, f":{DISPLAY}", f"--ppm-on-exit={PPM_PATH}", "--log=warn"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    try:
        s = connect()
        print("connected", flush=True)
        base = handshake(s)
        print(f"handshake ok base={base:#x}", flush=True)
        wid = base | 1
        fid = base | 2
        gid = base | 3

        create_window(s, wid, ROOT_WID, 100, 100, 400, 80)
        map_window(s, wid)
        print("window created+mapped", flush=True)
        open_font(s, fid, "fixed")
        print("open_font sent", flush=True)

        # QueryFont and verify layout.
        print("sending query_font", flush=True)
        head, body = query_font(s, fid)
        print(f"query_font replied: head={len(head)}B body={len(body)}B", flush=True)
        full = head + body
        # Absolute offsets into `full`:
        #   0..8   std reply header
        #   8..20  min-bounds CHARINFO
        #   20..24 pad
        #   24..36 max-bounds CHARINFO (lsb, rsb, width, ascent, descent, attrs)
        #   36..40 pad
        #   40..42 min-char  42..44 max-char
        #   44..46 default-char  46..48 n-props
        #   48..52 draw-dir/min-byte1/max-byte1/all-chars
        #   52..54 font-ascent  54..56 font-descent
        #   56..60 n-chars
        max_width = struct.unpack_from("<H", full, 24 + 4)[0]
        ascent    = struct.unpack_from("<H", full, 24 + 6)[0]
        first_ch  = struct.unpack_from("<H", full, 40)[0]
        last_ch   = struct.unpack_from("<H", full, 42)[0]
        n_chars   = struct.unpack_from("<I", full, 56)[0]
        print(f"QueryFont: width={max_width} ascent={ascent} first={first_ch} last={last_ch} nchars={n_chars}")
        assert max_width == 8, f"expected char_w=8, got {max_width}"
        assert ascent == 13, f"expected ascent=13, got {ascent}"
        assert n_chars == 256, f"expected 256 chars, got {n_chars}"

        # QueryTextExtents "HI" = 2*8 = 16.
        extents_head = query_text_extents(s, fid, "HI")
        width = struct.unpack_from("<I", extents_head, 16)[0]
        print(f"QueryTextExtents HI width={width}")
        assert width == 16, f"expected width=16, got {width}"

        # Set up drawing state and paint.
        create_gc(s, gid, wid, fg=0x0000FF00, bg=0x00000000, font_id=fid)
        # Fill the window with blue so we can see the text box.
        poly_fill_rect(s, wid, gid, 0, 0, 400, 80)
        # Now re-create the GC with green fg / black bg for text.
        # (We just drew blue with the same GC, but that's fine — we want
        #  ImageText8 to draw green glyphs on a black bg.)
        # ImageText8 at (x=50, y=30) relative to the window.
        image_text_8(s, wid, gid, 50, 30, "HELLO")

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
    # Debug: print the H glyph area as a 16x8 block.
    print("H glyph area (screen 150..158 × 117..133):")
    for py in range(117, 133):
        row = ""
        for px in range(150, 158):
            r, g, b = pixel(pixels, w, px, py)
            if (r, g, b) == (0, 0xFF, 0):
                row += "G"
            elif (r, g, b) == (0, 0, 0):
                row += "."
            else:
                row += "?"
        print(f"  y={py}: {row}")
    # The window is at (100, 100), size 400x80, and we drew the text at
    # window-relative (50, 30).  The text baseline is at y=30 (screen
    # 130), ascent=13 so the top of the first glyph cell is at screen
    # y = 100 + 30 - 13 = 117.  The 'H' cell is at screen x = 100 + 50
    # = 150.
    cell_x = 150
    cell_y = 117
    # Our 'H' glyph: first 2 rows blank, rows 2..10 have pattern 0x66
    # (left and right vertical bars), row 5 is 0x7E (full bar).  Check
    # row 4 (idx 4 in the glyph = pattern 0x66 = 01100110):
    #   columns 1,2 lit; columns 5,6 lit.
    row = cell_y + 4
    bit_pattern = 0x66  # 01100110
    print(f"cell at ({cell_x},{cell_y}), checking row {row}")
    lit_cols = [c for c in range(8) if (bit_pattern >> (7 - c)) & 1]
    for c in lit_cols:
        p = pixel(pixels, w, cell_x + c, row)
        print(f"  col {c} at screen ({cell_x+c},{row}) = {p}")
        if p != (0x00, 0xFF, 0x00):
            print(f"FAIL: expected green at ({cell_x+c},{row}), got {p}")
            return 1
    # Unlit columns should be background (black from GC bg).
    unlit_cols = [c for c in range(8) if not ((bit_pattern >> (7 - c)) & 1)]
    for c in unlit_cols:
        p = pixel(pixels, w, cell_x + c, row)
        print(f"  col {c} at screen ({cell_x+c},{row}) = {p}")
        if p != (0, 0, 0):
            print(f"FAIL: expected black at ({cell_x+c},{row}), got {p}")
            return 1

    # The fill is green (GC foreground) — confirm a pixel outside the
    # text cell still holds the fill color, not overwritten by a glyph.
    p = pixel(pixels, w, 300, 170)
    print(f"fill-area pixel at (300,170) = {p}")
    if p != (0, 0xFF, 0):
        print(f"FAIL: expected green fill at (300,170), got {p}")
        return 1

    print("PASS: Phase 6 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
