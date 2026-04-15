#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Phase 8 host smoke test: window-manager protocol.
#
# Uses TWO clients — a fake WM (wm) and an app (app) — talking to the
# same kxserver.  The test covers:
#
#  1. Cross-client event routing (Phase-8 requirement).
#  2. SubstructureRedirect pre-dispatch: wm selects
#     SUBSTRUCTURE_REDIRECT on root; app MapWindow on its child
#     window is intercepted and delivered as MapRequest to wm.
#  3. After wm MapWindow the child explicitly, both clients receive
#     a MapNotify (wm via SUBSTRUCTURE_NOTIFY, app via STRUCTURE_NOTIFY).
#  4. ReparentWindow: wm reparents app's window under a decoration
#     window, and a ReparentNotify reaches app.
#  5. SetSelectionOwner / GetSelectionOwner round trip.
#  6. SendEvent: wm sends a ClientMessage-style event to app; app
#     receives it with the 0x80 "synthesized" bit set.

import os, signal, socket, struct, subprocess, sys, time

BIN = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "target", "x86_64-unknown-linux-musl", "release", "kxserver",
)
DISPLAY = 95
PPM_PATH = "/tmp/kxserver-phase8.ppm"
ABSTRACT_PATH = f"/tmp/.X11-unix/X{DISPLAY}"
ROOT_WID = 0x20

# Event codes
EV_MAP_REQUEST    = 20
EV_MAP_NOTIFY     = 19
EV_REPARENT_NOTIFY = 21
EV_CONFIGURE_REQUEST = 23
EV_CLIENT_MESSAGE = 33

# Event mask bits
EM_STRUCTURE_NOTIFY     = 0x00020000
EM_SUBSTRUCTURE_NOTIFY  = 0x00080000
EM_SUBSTRUCTURE_REDIRECT= 0x00100000
EM_PROPERTY_CHANGE      = 0x00400000

# ── Wire helpers ────────────────────────────────────────────────────
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

def create_window_with_mask(s, wid, parent, x, y, w, h, event_mask=0):
    # CreateWindow with CWEventMask (bit 11, value mask bit 0x0800)
    if event_mask:
        body = struct.pack(
            "<IIhhHHHHIII",
            wid, parent, x, y, w, h, 0, 1, 0, 0x0800, event_mask,
        )
    else:
        body = struct.pack("<IIhhHHHHII",
            wid, parent, x, y, w, h, 0, 1, 0, 0)
    length = (4 + len(body)) // 4
    s.sendall(struct.pack("<BBH", 1, 0, length) + body)

def map_window(s, wid):
    s.sendall(struct.pack("<BBHI", 8, 0, 2, wid))

def reparent_window(s, wid, new_parent, x, y):
    body = struct.pack("<IIhh", wid, new_parent, x, y)
    s.sendall(struct.pack("<BBH", 7, 0, (4+len(body))//4) + body)

def change_window_attributes_event_mask(s, wid, mask):
    # ChangeWindowAttributes with CWEventMask bit
    body = struct.pack("<III", wid, 0x0800, mask)
    s.sendall(struct.pack("<BBH", 2, 0, (4+len(body))//4) + body)

def set_selection_owner(s, selection_atom, owner_wid, time):
    body = struct.pack("<III", owner_wid, selection_atom, time)
    s.sendall(struct.pack("<BBH", 22, 0, (4+len(body))//4) + body)

def get_selection_owner(s, selection_atom):
    body = struct.pack("<I", selection_atom)
    s.sendall(struct.pack("<BBH", 23, 0, (4+len(body))//4) + body)
    head = recv_all(s, 32)
    assert head[0] == 1, f"GetSelectionOwner bad reply: {head[:4].hex()}"
    return struct.unpack_from("<I", head, 8)[0]

def send_event(s, propagate, dest, event_mask, ev_block):
    # ev_block is 32 bytes.
    body = struct.pack("<II", dest, event_mask) + ev_block
    s.sendall(struct.pack("<BBH", 25, propagate, (4+len(body))//4) + body)

def intern_atom(s, name, only_if_exists=0):
    nlen = len(name)
    pad = (-nlen) & 3
    body = struct.pack("<HH", nlen, 0) + name.encode() + b"\0" * pad
    s.sendall(struct.pack("<BBH", 16, only_if_exists, (4+len(body))//4) + body)
    head = recv_all(s, 32)
    assert head[0] == 1, f"InternAtom bad reply: {head[:4].hex()}"
    return struct.unpack_from("<I", head, 8)[0]

def poll_events(s, timeout=0.3, max_n=8):
    """Read any pending events (non-blocking-ish)."""
    s.settimeout(timeout)
    events = []
    try:
        while len(events) < max_n:
            head = s.recv(32)
            if not head: break
            if len(head) < 32:
                # accumulate
                rest = recv_all(s, 32 - len(head), timeout=0.3)
                head = head + rest
            events.append(head)
    except (socket.timeout, BlockingIOError):
        pass
    finally:
        s.settimeout(None)
    return events

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
        # ── Open both clients ──
        wm = connect()
        wm_base = handshake(wm)
        app = connect()
        app_base = handshake(app)
        print(f"wm base={wm_base:#x}  app base={app_base:#x}")

        # ── (1) wm selects SUBSTRUCTURE_REDIRECT | SUBSTRUCTURE_NOTIFY on root
        change_window_attributes_event_mask(
            wm, ROOT_WID, EM_SUBSTRUCTURE_REDIRECT | EM_SUBSTRUCTURE_NOTIFY,
        )

        # ── (2) app creates + maps a child of root ──
        app_wid = app_base | 1
        create_window_with_mask(
            app, app_wid, ROOT_WID, 20, 40, 200, 100,
            event_mask=EM_STRUCTURE_NOTIFY,
        )
        # MapWindow should be intercepted → MapRequest to wm, not a
        # real map.  app should NOT see MapNotify.
        map_window(app, app_wid)

        # Let the server process.
        time.sleep(0.15)

        wm_events = poll_events(wm, timeout=0.3)
        app_events = poll_events(app, timeout=0.3)
        print(f"wm received {len(wm_events)} events")
        for ev in wm_events:
            print(f"  {ev[0] & 0x7F} (sent={bool(ev[0]&0x80)}) {ev[:12].hex()}")
        print(f"app received {len(app_events)} events")
        for ev in app_events:
            print(f"  {ev[0] & 0x7F} (sent={bool(ev[0]&0x80)}) {ev[:12].hex()}")

        wm_map_request = [ev for ev in wm_events if (ev[0] & 0x7F) == EV_MAP_REQUEST]
        if len(wm_map_request) != 1:
            failures.append(f"wm expected 1 MapRequest, got {len(wm_map_request)}")
        else:
            parent = struct.unpack_from("<I", wm_map_request[0], 4)[0]
            window = struct.unpack_from("<I", wm_map_request[0], 8)[0]
            if parent != ROOT_WID:
                failures.append(f"MapRequest parent={parent:#x} != root")
            if window != app_wid:
                failures.append(f"MapRequest window={window:#x} != app_wid")

        app_map_notify = [ev for ev in app_events if (ev[0] & 0x7F) == EV_MAP_NOTIFY]
        if app_map_notify:
            failures.append(f"app should not have received MapNotify, got {len(app_map_notify)}")

        # ── (3) wm MapWindow the same window explicitly ──
        # Since wm IS the redirect owner, the interception check sees
        # owner == c.id and takes the normal path.
        map_window(wm, app_wid)
        time.sleep(0.15)

        wm_events = poll_events(wm, timeout=0.3)
        app_events = poll_events(app, timeout=0.3)
        print(f"after wm MapWindow:")
        print(f"  wm events: {[(e[0]&0x7F) for e in wm_events]}")
        print(f"  app events: {[(e[0]&0x7F) for e in app_events]}")

        if not any((e[0] & 0x7F) == EV_MAP_NOTIFY for e in app_events):
            failures.append("app should have received MapNotify after wm mapped")
        if not any((e[0] & 0x7F) == EV_MAP_NOTIFY for e in wm_events):
            failures.append("wm should have received MapNotify (substructure) after mapping")

        # ── (4) wm creates a "frame" window and reparents app's window into it ──
        frame_wid = wm_base | 1
        create_window_with_mask(
            wm, frame_wid, ROOT_WID, 0, 0, 300, 200,
            event_mask=EM_STRUCTURE_NOTIFY | EM_SUBSTRUCTURE_NOTIFY,
        )
        # Drain any wm events from the frame creation.
        poll_events(wm, timeout=0.1)
        reparent_window(wm, app_wid, frame_wid, 10, 25)
        time.sleep(0.15)

        wm_events = poll_events(wm, timeout=0.3)
        app_events = poll_events(app, timeout=0.3)
        print(f"after Reparent:")
        print(f"  wm events: {[(e[0]&0x7F) for e in wm_events]}")
        print(f"  app events: {[(e[0]&0x7F) for e in app_events]}")

        app_reparents = [e for e in app_events if (e[0] & 0x7F) == EV_REPARENT_NOTIFY]
        if len(app_reparents) != 1:
            failures.append(f"app expected 1 ReparentNotify, got {len(app_reparents)}")
        else:
            event_w = struct.unpack_from("<I", app_reparents[0], 4)[0]
            win = struct.unpack_from("<I", app_reparents[0], 8)[0]
            new_parent = struct.unpack_from("<I", app_reparents[0], 12)[0]
            rx = struct.unpack_from("<h", app_reparents[0], 16)[0]
            ry = struct.unpack_from("<h", app_reparents[0], 18)[0]
            print(f"  app ReparentNotify: event={event_w:#x} win={win:#x} parent={new_parent:#x} @({rx},{ry})")
            if event_w != app_wid or win != app_wid or new_parent != frame_wid:
                failures.append(f"ReparentNotify fields wrong")
            if (rx, ry) != (10, 25):
                failures.append(f"ReparentNotify pos ({rx},{ry}) != (10,25)")

        # wm should receive ReparentNotify too (substructure notify on the new parent).
        wm_reparents = [e for e in wm_events if (e[0] & 0x7F) == EV_REPARENT_NOTIFY]
        if not wm_reparents:
            failures.append("wm should have received ReparentNotify on frame (substructure)")

        # ── (5) Selection owner round-trip ──
        sel_atom = intern_atom(wm, "PRIMARY")
        print(f"PRIMARY atom = {sel_atom}")
        set_selection_owner(wm, sel_atom, frame_wid, 0)
        owner = get_selection_owner(app, sel_atom)
        print(f"GetSelectionOwner PRIMARY → {owner:#x}")
        if owner != frame_wid:
            failures.append(f"selection owner readback {owner:#x} != {frame_wid:#x}")
        # Clear it.
        set_selection_owner(wm, sel_atom, 0, 0)
        owner = get_selection_owner(app, sel_atom)
        if owner != 0:
            failures.append(f"selection owner after clear = {owner:#x} != 0")

        # ── (6) SendEvent: wm sends a ClientMessage to app's window ──
        # Build a ClientMessage 32-byte block.
        cm = bytearray(32)
        cm[0] = EV_CLIENT_MESSAGE
        cm[1] = 32  # format
        struct.pack_into("<I", cm, 4, app_wid)   # window
        struct.pack_into("<I", cm, 8, 0x1234)    # type atom (dummy)
        # Drain any stale events on app.
        poll_events(app, timeout=0.1)
        # app must be listening for the event — it selected
        # STRUCTURE_NOTIFY on its window but not PROPERTY_CHANGE.
        # SendEvent with event_mask=0 delivers to ALL listeners on
        # the window regardless of mask, so use that.
        send_event(wm, 0, app_wid, 0, bytes(cm))
        time.sleep(0.15)
        app_events = poll_events(app, timeout=0.3)
        client_msgs = [e for e in app_events if (e[0] & 0x7F) == EV_CLIENT_MESSAGE]
        print(f"app ClientMessage events: {len(client_msgs)}")
        if not client_msgs:
            failures.append("app did not receive SendEvent'd ClientMessage")
        else:
            cm_recv = client_msgs[0]
            if cm_recv[0] & 0x80 == 0:
                failures.append("SendEvent'd event missing sent bit (0x80)")
            target = struct.unpack_from("<I", cm_recv, 4)[0]
            atom = struct.unpack_from("<I", cm_recv, 8)[0]
            if target != app_wid:
                failures.append(f"ClientMessage window={target:#x} != app_wid")
            if atom != 0x1234:
                failures.append(f"ClientMessage type={atom:#x} != 0x1234")

        wm.close()
        app.close()
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
    print("PASS: Phase 8 smoke test")
    return 0

if __name__ == "__main__":
    sys.exit(main())
