#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 9 host smoke test: extensions + BIG-REQUESTS + input routing.
#
# Three sub-tests:
#
#   (A) Extension negotiation:
#       - QueryExtension("BIG-REQUESTS")   → present=1, major=128
#       - QueryExtension("RENDER")         → present=0
#       - QueryExtension("XKEYBOARD")      → present=0
#       - ListExtensions                   → "BIG-REQUESTS"
#
#   (B) BIG-REQUESTS:
#       - BigReqEnable (major 128, minor 0) → reply with max-len
#       - Send a PolyFillRectangle whose length is forced to 0 and
#         whose extended 32-bit length is used instead.  The server
#         must decode the extended length, dispatch normally, and
#         fill the framebuffer.  Verify via the dumped PPM.
#
#   (C) Input routing via --inject:
#       - kxserver is launched with --inject=motion:100:50 (abs move
#         from screen center), --inject=key:38:down (press 'a'),
#         --inject=button:1:down (LMB press).
#       - Test client creates + maps a window that covers the
#         target pointer location, selects POINTER_MOTION | KEY_PRESS
#         | BUTTON_PRESS on it.
#       - After injection, the test reads events from the client
#         socket and verifies: MotionNotify with root_x,root_y at
#         (612,434), KeyPress keycode=38, ButtonPress button=1.

import os, signal, socket, struct, subprocess, sys, time

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "target", "x86_64-unknown-linux-musl", "release", "kxserver",
)
DISPLAY = 94
PPM_PATH = "/tmp/kxserver-phase9.ppm"
ABSTRACT_PATH = f"/tmp/.X11-unix/X{DISPLAY}"
ROOT_WID = 0x20

EM_KEY_PRESS     = 0x00000001
EM_BUTTON_PRESS  = 0x00000004
EM_POINTER_MOTION= 0x00000040

EV_KEY_PRESS      = 2
EV_BUTTON_PRESS   = 4
EV_MOTION_NOTIFY  = 6

def connect():
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    for _ in range(50):
        try:
            s.connect("\0" + ABSTRACT_PATH); return s
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
    nlen = len(nb)
    pad = (-nlen) & 3
    body = struct.pack("<HH", nlen, 0) + nb + b"\0" * pad
    s.sendall(struct.pack("<BBH", 98, 0, (4+len(body))//4) + body)
    return recv_reply(s)

def list_extensions(s):
    s.sendall(struct.pack("<BBH", 99, 0, 1))
    return recv_reply(s)

def big_req_enable(s):
    s.sendall(struct.pack("<BBH", 128, 0, 1))
    return recv_reply(s)

def create_window_event_mask(s, wid, parent, x, y, w, h, event_mask):
    body = struct.pack("<IIhhHHHHIII",
        wid, parent, x, y, w, h, 0, 1, 0, 0x0800, event_mask)
    s.sendall(struct.pack("<BBH", 1, 0, (4+len(body))//4) + body)

def map_window(s, wid):
    s.sendall(struct.pack("<BBHI", 8, 0, 2, wid))

def create_gc_fg(s, gid, drawable, fg):
    mask = 0x04
    body = struct.pack("<IIII", gid, drawable, mask, fg)
    s.sendall(struct.pack("<BBH", 55, 0, (4+len(body))//4) + body)

def poly_fill_rect_big(s, wid, gid, x, y, w, h):
    # Force BIG-REQUESTS length=0 form: put 0 in the 16-bit length
    # field and the real length (in words) in a 32-bit slot after it.
    # Body: wid(4) gc(4) x(2) y(2) w(2) h(2) = 16 bytes body
    # Total without big = 4 hdr + 16 body = 20 bytes = 5 words
    # BIG-REQUESTS total = 8 hdr + 16 body = 24 bytes = 6 words
    body = struct.pack("<IIhhHH", wid, gid, x, y, w, h)
    total_bytes = 4 + 4 + len(body)   # opcode hdr + extended length word + body
    assert total_bytes % 4 == 0
    total_words = total_bytes // 4
    # opcode=70 (PolyFillRectangle), data=0, length=0 (BIG-REQUESTS)
    header = struct.pack("<BBH", 70, 0, 0)
    ext_len = struct.pack("<I", total_words)
    s.sendall(header + ext_len + body)

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

def poll_events(s, timeout=0.3, max_n=16):
    s.settimeout(timeout)
    events = []
    buf = b""
    try:
        while len(events) < max_n:
            chunk = s.recv(32 - len(buf))
            if not chunk: break
            buf += chunk
            if len(buf) == 32:
                events.append(buf)
                buf = b""
    except (socket.timeout, BlockingIOError):
        pass
    finally:
        s.settimeout(None)
    return events

def run_extensions_and_bigreq_test():
    print("=== (A) (B) extensions + BIG-REQUESTS ===")
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

        # (A) Extension negotiation
        r = query_extension(s, "BIG-REQUESTS")
        present = r[8]
        major = r[9]
        print(f"BIG-REQUESTS: present={present} major={major}")
        if present != 1: failures.append("BIG-REQUESTS not present")
        if major != 128: failures.append(f"BIG-REQUESTS major={major} != 128")

        # Post-phase-10, RENDER is advertised; pre-phase-10 it wasn't.
        # Just verify the request round-trips and returns a valid byte.
        r = query_extension(s, "RENDER")
        print(f"RENDER: present={r[8]}")

        r = query_extension(s, "XKEYBOARD")
        print(f"XKEYBOARD: present={r[8]}")
        if r[8] != 0: failures.append("XKEYBOARD should be absent")

        r = list_extensions(s)
        n_names = r[1]
        print(f"ListExtensions: n_names={n_names}")
        if n_names < 1: failures.append(f"ListExtensions n={n_names} < 1")
        # First name starts at byte 32: length byte + name bytes
        first_len = r[32]
        first = bytes(r[33:33+first_len])
        print(f"  first: {first!r}")
        if first != b"BIG-REQUESTS":
            failures.append(f"first name = {first!r}")

        # (B) BigReqEnable
        r = big_req_enable(s)
        max_len = struct.unpack_from("<I", r, 8)[0]
        print(f"BigReqEnable max_len={max_len} words")
        if max_len < 262144:
            failures.append(f"max_len={max_len} too small")

        # Create a window and fill it via BIG-REQUESTS
        wid = base | 1
        gid = base | 2
        create_window_event_mask(s, wid, ROOT_WID, 50, 50, 300, 200, 0)
        map_window(s, wid)
        create_gc_fg(s, gid, wid, 0x0000FF00)
        poly_fill_rect_big(s, wid, gid, 0, 0, 300, 200)

        s.close()
        time.sleep(0.15)
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            out, _ = proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            out, _ = proc.communicate()

    if os.path.exists(PPM_PATH):
        w, h, pix = read_ppm(PPM_PATH)
        # Window at (50,50), size 300x200. Check a pixel in the middle
        # is green.
        p = pixel(pix, w, 200, 150)
        print(f"center pixel (200,150) = {p}")
        if p != (0, 0xFF, 0):
            failures.append(f"BIG-REQUESTS fill pixel ({p}) not green")
    else:
        failures.append("no PPM after BIG-REQUESTS test")

    return failures

def run_input_injection_test():
    print("=== (C) input event routing via --inject ===")
    proc = subprocess.Popen(
        [BIN, f":{DISPLAY+1}", "--log=warn",
         "--inject=motion:100:50",      # move pointer +100, +50 from center (512,384) → (612, 434)
         "--inject=button:1:down",
         "--inject=key:38:down",        # 'a'
        ],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    )
    failures = []
    try:
        # Connect and create a window big enough to contain (612,434)
        # under the pointer.
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        for _ in range(50):
            try:
                s.connect("\0" + f"/tmp/.X11-unix/X{DISPLAY+1}"); break
            except (FileNotFoundError, ConnectionRefusedError):
                time.sleep(0.05)
        base = handshake(s)
        wid = base | 1

        # Big window covering (612,434): anchor at (100,100), size 800x500
        # so (612,434) maps to (512, 334) inside.
        create_window_event_mask(
            s, wid, ROOT_WID, 100, 100, 800, 500,
            EM_KEY_PRESS | EM_BUTTON_PRESS | EM_POINTER_MOTION,
        )
        map_window(s, wid)
        # Take focus so KeyPress events route to this window.
        # SetInputFocus(wid, revert_to=Parent=2, time=0)
        s.sendall(struct.pack("<BBHII", 42, 2, 3, wid, 0))
        # NoOperation as a sync marker — kxserver arms the --inject
        # trigger here, so all preceding setup requests are
        # guaranteed to have been processed before the injected
        # events route.
        s.sendall(struct.pack("<BBH", 127, 0, 1))

        # Wait for poll cycle to fire injections.
        time.sleep(0.3)
        events = poll_events(s, timeout=0.5, max_n=16)
        print(f"received {len(events)} events")
        codes = [e[0] & 0x7F for e in events]
        print(f"  codes: {codes}")

        motions = [e for e in events if (e[0] & 0x7F) == EV_MOTION_NOTIFY]
        buttons = [e for e in events if (e[0] & 0x7F) == EV_BUTTON_PRESS]
        keys    = [e for e in events if (e[0] & 0x7F) == EV_KEY_PRESS]

        if not motions:
            failures.append("no MotionNotify received")
        else:
            m = motions[0]
            root_x = struct.unpack_from("<H", m, 20)[0]
            root_y = struct.unpack_from("<H", m, 22)[0]
            win_x  = struct.unpack_from("<h", m, 24)[0]
            win_y  = struct.unpack_from("<h", m, 26)[0]
            print(f"  Motion: root=({root_x},{root_y}) win=({win_x},{win_y})")
            if (root_x, root_y) != (612, 434):
                failures.append(f"motion root ({root_x},{root_y}) != (612,434)")
            if (win_x, win_y) != (512, 334):
                failures.append(f"motion win ({win_x},{win_y}) != (512,334)")

        if not buttons:
            failures.append("no ButtonPress received")
        else:
            b = buttons[0]
            print(f"  Button: {b[1]}")
            if b[1] != 1:
                failures.append(f"button detail {b[1]} != 1")

        if not keys:
            failures.append("no KeyPress received")
        else:
            k = keys[0]
            print(f"  Key: keycode={k[1]}")
            if k[1] != 38:
                failures.append(f"key detail {k[1]} != 38")

        s.close()
        time.sleep(0.1)
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.communicate()
    return failures

def main():
    fail1 = run_extensions_and_bigreq_test()
    fail2 = run_input_injection_test()
    all_fails = fail1 + fail2
    if all_fails:
        print("FAIL:")
        for f in all_fails:
            print(" ", f)
        return 1
    print("PASS: Phase 9 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
