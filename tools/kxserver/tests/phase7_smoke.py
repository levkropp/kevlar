#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 7 host smoke test: input wire-protocol surface.
#
# No real device bytes are involved — the Kevlar-side /dev/input
# integration is a follow-up ticket.  This test exercises the
# handlers:
#   GetInputFocus (43), SetInputFocus (42), GetInputFocus again,
#   QueryPointer (38), WarpPointer (41), QueryPointer again,
#   GetKeyboardMapping (101), GetModifierMapping (119),
#   GetPointerMapping (117), QueryKeymap (44),
#   GetKeyboardControl (103), GrabPointer (26) → Success.

import os, signal, socket, struct, subprocess, sys, time

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "target",
    "x86_64-unknown-linux-musl",
    "release",
    "kxserver",
)
DISPLAY = 96
PPM_PATH = "/tmp/kxserver-phase7.ppm"
ABSTRACT_PATH = f"/tmp/.X11-unix/X{DISPLAY}"
ROOT_WID = 0x20

# Screen dimensions (matches setup::SCREEN_WIDTH/HEIGHT)
SCREEN_W, SCREEN_H = 1024, 768

# ── Wire helpers ────────────────────────────────────────────────────
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
    assert head[0] == 1
    extra = struct.unpack_from("<H", head, 6)[0]
    body = recv_all(s, extra * 4)
    return struct.unpack_from("<I", body, 4)[0]

def create_window(s, wid, parent, x, y, w, h):
    body = struct.pack("<IIhhHHHHII", wid, parent, x, y, w, h, 0, 1, 0, 0)
    s.sendall(struct.pack("<BBH", 1, 0, (4+len(body))//4) + body)

def map_window(s, wid):
    s.sendall(struct.pack("<BBHI", 8, 0, 2, wid))

def recv_reply(s):
    head = recv_all(s, 32)
    assert head[0] == 1, f"not a reply: {head[:4].hex()}"
    extra = struct.unpack_from("<I", head, 4)[0]
    body = recv_all(s, extra * 4)
    return head + body

# ── Request senders ────────────────────────────────────────────────
def req_get_input_focus(s):
    s.sendall(struct.pack("<BBH", 43, 0, 1))
    return recv_reply(s)

def req_set_input_focus(s, wid, revert_to):
    body = struct.pack("<II", wid, 0)  # focus, time
    s.sendall(struct.pack("<BBH", 42, revert_to, (4+len(body))//4) + body)

def req_query_pointer(s, wid):
    s.sendall(struct.pack("<BBHI", 38, 0, 2, wid))
    return recv_reply(s)

def req_warp_pointer(s, src, dst, sx, sy, sw, sh, dx, dy):
    body = struct.pack("<IIhhHHhh", src, dst, sx, sy, sw, sh, dx, dy)
    s.sendall(struct.pack("<BBH", 41, 0, (4+len(body))//4) + body)

def req_get_keyboard_mapping(s, first, count):
    body = struct.pack("<BBxx", first, count)
    s.sendall(struct.pack("<BBH", 101, 0, (4+len(body))//4) + body)
    return recv_reply(s)

def req_get_modifier_mapping(s):
    s.sendall(struct.pack("<BBH", 119, 0, 1))
    return recv_reply(s)

def req_get_pointer_mapping(s):
    s.sendall(struct.pack("<BBH", 117, 0, 1))
    return recv_reply(s)

def req_query_keymap(s):
    s.sendall(struct.pack("<BBH", 44, 0, 1))
    return recv_reply(s)

def req_get_keyboard_control(s):
    s.sendall(struct.pack("<BBH", 103, 0, 1))
    return recv_reply(s)

def req_grab_pointer(s, wid):
    body = struct.pack("<IHBBIII", wid, 0, 1, 1, 0, 0, 0)
    s.sendall(struct.pack("<BBH", 26, 0, (4+len(body))//4) + body)
    return recv_reply(s)

# ── Main ────────────────────────────────────────────────────────────
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
        wid = base | 1
        create_window(s, wid, ROOT_WID, 10, 20, 200, 100)
        map_window(s, wid)

        # ── GetInputFocus: should default to root + revert-to Parent ──
        r = req_get_input_focus(s)
        revert_to = r[1]
        focus = struct.unpack_from("<I", r, 8)[0]
        print(f"GetInputFocus: focus={focus:#x} revert={revert_to}")
        if focus != ROOT_WID:
            failures.append(f"GetInputFocus default focus={focus:#x} != root")
        if revert_to != 2:
            failures.append(f"GetInputFocus default revert={revert_to} != 2 (Parent)")

        # ── SetInputFocus to our window, revert-to PointerRoot ──
        req_set_input_focus(s, wid, 1)
        r = req_get_input_focus(s)
        revert_to = r[1]
        focus = struct.unpack_from("<I", r, 8)[0]
        print(f"after SetInputFocus: focus={focus:#x} revert={revert_to}")
        if focus != wid:
            failures.append(f"SetInputFocus didn't stick: focus={focus:#x} want {wid:#x}")
        if revert_to != 1:
            failures.append(f"SetInputFocus revert={revert_to} want 1")

        # ── QueryPointer at center (default InputState has pointer at 512,384) ──
        r = req_query_pointer(s, wid)
        same_screen = r[1]
        root = struct.unpack_from("<I", r, 8)[0]
        root_x = struct.unpack_from("<H", r, 16)[0]
        root_y = struct.unpack_from("<H", r, 18)[0]
        win_x  = struct.unpack_from("<h", r, 20)[0]
        win_y  = struct.unpack_from("<h", r, 22)[0]
        print(f"QueryPointer: root=({root_x},{root_y}) win=({win_x},{win_y}) same_screen={same_screen}")
        if (root_x, root_y) != (SCREEN_W // 2, SCREEN_H // 2):
            failures.append(f"default pointer ({root_x},{root_y}) != screen center")
        # Window origin is (10, 20) so win_x = 512 - 10 = 502, win_y = 384 - 20 = 364.
        if (win_x, win_y) != (SCREEN_W // 2 - 10, SCREEN_H // 2 - 20):
            failures.append(f"win-relative ({win_x},{win_y}) wrong")
        if root != ROOT_WID:
            failures.append(f"root in QueryPointer reply = {root:#x}")

        # ── WarpPointer dst=root to absolute (100, 150) ──
        req_warp_pointer(s, 0, ROOT_WID, 0, 0, 0, 0, 100, 150)
        r = req_query_pointer(s, wid)
        root_x = struct.unpack_from("<H", r, 16)[0]
        root_y = struct.unpack_from("<H", r, 18)[0]
        print(f"after Warp(root, 100, 150): pointer=({root_x},{root_y})")
        # abs_origin(root) = (0, 0) so final = (100, 150).
        if (root_x, root_y) != (100, 150):
            failures.append(f"Warp abs → ({root_x},{root_y}) != (100, 150)")

        # ── WarpPointer relative (src=0, dst=0, dx=5, dy=-10) ──
        req_warp_pointer(s, 0, 0, 0, 0, 0, 0, 5, -10)
        r = req_query_pointer(s, wid)
        root_x = struct.unpack_from("<H", r, 16)[0]
        root_y = struct.unpack_from("<H", r, 18)[0]
        print(f"after Warp(rel, +5, -10): pointer=({root_x},{root_y})")
        if (root_x, root_y) != (105, 140):
            failures.append(f"Warp rel → ({root_x},{root_y}) != (105, 140)")

        # ── GetKeyboardMapping for keycode 38 ('a' in US QWERTY) ──
        # evdev KEY_A=30, X11 keycode = 38.
        r = req_get_keyboard_mapping(s, 38, 1)
        per = r[1]
        print(f"GetKeyboardMapping per_keycode={per}")
        if per != 2:
            failures.append(f"per_keycode={per} != 2")
        sym_lower = struct.unpack_from("<I", r, 32)[0]
        sym_upper = struct.unpack_from("<I", r, 36)[0]
        print(f"  keycode 38: lower={sym_lower:#x} upper={sym_upper:#x}")
        if sym_lower != ord('a'):
            failures.append(f"keycode 38 lower={sym_lower:#x} != 'a'")
        if sym_upper != ord('A'):
            failures.append(f"keycode 38 upper={sym_upper:#x} != 'A'")

        # ── Keycode 9 (escape, evdev 1) ──
        r = req_get_keyboard_mapping(s, 9, 1)
        sym = struct.unpack_from("<I", r, 32)[0]
        print(f"  keycode 9: {sym:#x}")
        if sym != 0xFF1B:
            failures.append(f"keycode 9 = {sym:#x} != XK_Escape")

        # ── GetModifierMapping: shift in slot 0 ──
        r = req_get_modifier_mapping(s)
        per_mod = r[1]
        print(f"GetModifierMapping per_mod={per_mod}")
        if per_mod != 2:
            failures.append(f"per_mod={per_mod} != 2")
        # Row 0 = Shift: keycodes for Shift_L (42+8=50) and Shift_R (54+8=62)
        shift0 = r[32]
        shift1 = r[33]
        print(f"  shift: {shift0}, {shift1}")
        if shift0 != 50 or shift1 != 62:
            failures.append(f"shift map wrong: {shift0},{shift1}")
        # Row 2 = Control: 29+8=37, 97+8=105
        ctrl0 = r[32 + 2 * per_mod]
        ctrl1 = r[32 + 2 * per_mod + 1]
        print(f"  control: {ctrl0}, {ctrl1}")
        if ctrl0 != 37 or ctrl1 != 105:
            failures.append(f"control map wrong: {ctrl0},{ctrl1}")

        # ── GetPointerMapping: 5 buttons, identity ──
        r = req_get_pointer_mapping(s)
        n = r[1]
        print(f"GetPointerMapping n={n}")
        if n != 5:
            failures.append(f"pointer_map n={n} != 5")
        for i in range(5):
            if r[32 + i] != i + 1:
                failures.append(f"pointer_map[{i}] = {r[32+i]} != {i+1}")

        # ── QueryKeymap: 32 bytes of zero ──
        r = req_query_keymap(s)
        print(f"QueryKeymap reply bytes 32..40: {r[32:40].hex()}")
        if any(b != 0 for b in r[32:64]):
            failures.append("QueryKeymap not all-zero")

        # ── GetKeyboardControl: sanity ──
        r = req_get_keyboard_control(s)
        auto_repeat = r[1]
        led = struct.unpack_from("<I", r, 8)[0]
        bell_pitch = struct.unpack_from("<H", r, 14)[0]
        print(f"GetKeyboardControl auto_repeat={auto_repeat} led={led} bell_pitch={bell_pitch}")
        if auto_repeat != 1:
            failures.append(f"auto_repeat={auto_repeat}")
        if bell_pitch != 400:
            failures.append(f"bell_pitch={bell_pitch}")

        # ── GrabPointer → GrabSuccess (status 0) ──
        r = req_grab_pointer(s, wid)
        status = r[1]
        print(f"GrabPointer status={status}")
        if status != 0:
            failures.append(f"GrabPointer status={status} != 0")

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
    print("PASS: Phase 7 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
