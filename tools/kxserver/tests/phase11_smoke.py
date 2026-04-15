#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 11 smoke test: XFIXES extension + region clipping of
# RENDER operations.
#
# Flow:
#   (A) QueryExtension(XFIXES) + QueryVersion → 5.0
#   (B) CreateRegion with two rects, FetchRegion, assert rects back
#   (C) Intersect two regions, FetchRegion on the result
#   (D) RegionExtents + FetchRegion round-trip
#   (E) Subtract a hole from a region, verify via FetchRegion
#   (F) SetPictureClipRegion on a RENDER picture, then
#       FillRectangles Src-blue across the whole window — verify
#       only the clipped region went blue and the rest stayed red
#   (G) GetCursorImage returns an 8x8 alternating bitmap at the
#       current pointer position

import os, signal, socket, struct, subprocess, sys, time

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "target", "x86_64-unknown-linux-musl", "release", "kxserver",
)
DISPLAY = 92
ABSTRACT = f"/tmp/.X11-unix/X{DISPLAY}"
ROOT_WID = 0x20

RENDER_MAJOR = 129
XFIXES_MAJOR = 130
PICTFMT_A8R8G8B8 = 0x10
PICT_OP_SRC = 1

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

# ── RENDER ─────────────────────────────────────────────────────────
def render_create_picture(s, pid, drawable, fmt):
    body = struct.pack("<IIII", pid, drawable, fmt, 0)
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 4, (4+len(body))//4) + body)

def render_fill_rectangles(s, op, dst_pid, r, g, b, a, rects):
    # op at byte 4 with 3 bytes of padding (extension wire layout).
    body = struct.pack("<B3xIHHHH", op, dst_pid, r, g, b, a)
    for (x, y, w, h) in rects:
        body += struct.pack("<hhHH", x, y, w, h)
    s.sendall(struct.pack("<BBH", RENDER_MAJOR, 26, (4+len(body))//4) + body)

# ── XFIXES ─────────────────────────────────────────────────────────
def xfixes_query_version(s, major=5, minor=0):
    body = struct.pack("<II", major, minor)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 0, (4+len(body))//4) + body)
    return recv_reply(s)

def xfixes_create_region(s, rid, rects):
    body = struct.pack("<I", rid)
    for (x, y, w, h) in rects:
        body += struct.pack("<hhHH", x, y, w, h)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 5, (4+len(body))//4) + body)

def xfixes_destroy_region(s, rid):
    body = struct.pack("<I", rid)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 10, (4+len(body))//4) + body)

def xfixes_set_region(s, rid, rects):
    body = struct.pack("<I", rid)
    for (x, y, w, h) in rects:
        body += struct.pack("<hhHH", x, y, w, h)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 11, (4+len(body))//4) + body)

def xfixes_copy_region(s, src, dst):
    body = struct.pack("<II", src, dst)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 12, (4+len(body))//4) + body)

def xfixes_combine(s, minor, src1, src2, dst):
    body = struct.pack("<III", src1, src2, dst)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, minor, (4+len(body))//4) + body)

def xfixes_union(s, a, b, dst):      xfixes_combine(s, 13, a, b, dst)
def xfixes_intersect(s, a, b, dst):  xfixes_combine(s, 14, a, b, dst)
def xfixes_subtract(s, a, b, dst):   xfixes_combine(s, 15, a, b, dst)

def xfixes_translate_region(s, rid, dx, dy):
    body = struct.pack("<Ihh", rid, dx, dy)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 17, (4+len(body))//4) + body)

def xfixes_region_extents(s, src, dst):
    body = struct.pack("<II", src, dst)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 18, (4+len(body))//4) + body)

def xfixes_fetch_region(s, rid):
    body = struct.pack("<I", rid)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 19, (4+len(body))//4) + body)
    return recv_reply(s)

def xfixes_set_picture_clip_region(s, pid, clip_x, clip_y, rid):
    body = struct.pack("<IhhI", pid, clip_x, clip_y, rid)
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 22, (4+len(body))//4) + body)

def xfixes_get_cursor_image(s):
    s.sendall(struct.pack("<BBH", XFIXES_MAJOR, 4, 1))
    return recv_reply(s)

def parse_fetch_region_reply(r):
    # Header is 32 bytes; extents at bytes 8..16; rectangles at 32..
    ext = struct.unpack_from("<hhHH", r, 8)
    rects = []
    body = r[32:]
    nrects = len(body) // 8
    for i in range(nrects):
        rects.append(struct.unpack_from("<hhHH", body, i*8))
    return ext, rects

def main():
    proc = subprocess.Popen(
        [BIN, f":{DISPLAY}", "--log=warn"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    failures = []
    try:
        s = connect()
        base = handshake(s)
        print(f"base={base:#x}")

        # (A) extension present + QueryVersion
        r = query_extension(s, "XFIXES")
        print(f"XFIXES: present={r[8]} major={r[9]}")
        if r[8] != 1 or r[9] != XFIXES_MAJOR:
            failures.append(f"XFIXES missing: present={r[8]} major={r[9]}")
        r = xfixes_query_version(s)
        mj = struct.unpack_from("<I", r, 8)[0]
        mn = struct.unpack_from("<I", r, 12)[0]
        print(f"XFIXES QueryVersion → {mj}.{mn}")
        if (mj, mn) != (5, 0):
            failures.append(f"version {mj}.{mn} != 5.0")

        # (B) CreateRegion + FetchRegion round-trip
        rid_a = base | 1
        xfixes_create_region(s, rid_a, [(0, 0, 10, 10), (20, 0, 10, 10)])
        r = xfixes_fetch_region(s, rid_a)
        ext, rects = parse_fetch_region_reply(r)
        print(f"FetchRegion rid_a: extents={ext} rects={rects}")
        if ext != (0, 0, 30, 10):
            failures.append(f"rid_a extents {ext} wrong")
        if set(rects) != {(0, 0, 10, 10), (20, 0, 10, 10)}:
            failures.append(f"rid_a rects {rects} wrong")

        # (C) IntersectRegion
        rid_b = base | 2
        rid_c = base | 3
        xfixes_create_region(s, rid_b, [(5, 0, 20, 5)])
        xfixes_create_region(s, rid_c, [])
        xfixes_intersect(s, rid_a, rid_b, rid_c)
        r = xfixes_fetch_region(s, rid_c)
        ext, rects = parse_fetch_region_reply(r)
        print(f"IntersectRegion → rects={rects}")
        # Intersection: (5,0,5,5) and (20,0,5,5)
        if set(rects) != {(5, 0, 5, 5), (20, 0, 5, 5)}:
            failures.append(f"intersect rects {rects} wrong")

        # (D) RegionExtents
        rid_d = base | 4
        xfixes_create_region(s, rid_d, [])
        xfixes_region_extents(s, rid_a, rid_d)
        r = xfixes_fetch_region(s, rid_d)
        ext, rects = parse_fetch_region_reply(r)
        print(f"RegionExtents → rects={rects}")
        if set(rects) != {(0, 0, 30, 10)}:
            failures.append(f"extents rects {rects} wrong")

        # (E) SubtractRegion: a big rect minus a hole produces 4 rects
        rid_big  = base | 5
        rid_hole = base | 6
        rid_res  = base | 7
        xfixes_create_region(s, rid_big,  [(0, 0, 10, 10)])
        xfixes_create_region(s, rid_hole, [(3, 3, 4, 4)])
        xfixes_create_region(s, rid_res,  [])
        xfixes_subtract(s, rid_big, rid_hole, rid_res)
        r = xfixes_fetch_region(s, rid_res)
        _, rects = parse_fetch_region_reply(r)
        print(f"SubtractRegion rects={rects}")
        expected = {(0, 0, 10, 3), (0, 7, 10, 3), (0, 3, 3, 4), (7, 3, 3, 4)}
        if set(rects) != expected:
            failures.append(f"subtract rects {rects} != {expected}")

        # (F) Picture clip region interaction
        wid = base | 8
        gid = base | 9
        dst_pic = base | 10
        clip_rid = base | 11
        create_window(s, wid, ROOT_WID, 50, 50, 100, 100)
        map_window(s, wid)
        # Core PolyFillRectangle paints the entire window red.
        create_gc(s, gid, wid, 0x00FF0000)
        poly_fill_rect(s, wid, gid, 0, 0, 100, 100)
        # Create RENDER picture + a clip region covering only (10,10, 20, 20).
        render_create_picture(s, dst_pic, wid, PICTFMT_A8R8G8B8)
        xfixes_create_region(s, clip_rid, [(10, 10, 20, 20)])
        xfixes_set_picture_clip_region(s, dst_pic, 0, 0, clip_rid)
        # FillRectangles Src-blue across the whole window.
        render_fill_rectangles(
            s, PICT_OP_SRC, dst_pic,
            r=0x0000, g=0x0000, b=0xFFFF, a=0xFFFF,
            rects=[(0, 0, 100, 100)],
        )
        # Readback: inside the clip → blue; outside → still red.
        r = get_image(s, wid, 15, 15, 1, 1)
        inside = struct.unpack_from("<I", r, 32)[0] & 0xFFFFFF
        r = get_image(s, wid, 40, 40, 1, 1)
        outside = struct.unpack_from("<I", r, 32)[0] & 0xFFFFFF
        r = get_image(s, wid, 5, 5, 1, 1)
        outside2 = struct.unpack_from("<I", r, 32)[0] & 0xFFFFFF
        print(f"clip pixels: inside={inside:06x} outside={outside:06x} corner={outside2:06x}")
        if inside != 0x0000FF:
            failures.append(f"inside-clip pixel {inside:06x} not blue")
        if outside != 0xFF0000:
            failures.append(f"outside-clip pixel {outside:06x} not red")
        if outside2 != 0xFF0000:
            failures.append(f"corner-outside pixel {outside2:06x} not red")

        # (G) GetCursorImage
        r = xfixes_get_cursor_image(s)
        img_w = struct.unpack_from("<H", r, 12)[0]
        img_h = struct.unpack_from("<H", r, 14)[0]
        print(f"GetCursorImage → {img_w}x{img_h}")
        if (img_w, img_h) != (8, 8):
            failures.append(f"cursor size {img_w}x{img_h} != 8x8")

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

    if failures:
        print("FAIL:")
        for f in failures:
            print(" ", f)
        return 1
    print("PASS: Phase 11 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
