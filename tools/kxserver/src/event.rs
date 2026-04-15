// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Event wire-format builders.
//
// Every core X11 event is exactly 32 bytes.  The first byte is the
// event code (with bit 0x80 set if the event was synthesized via
// SendEvent).  Byte 1 is event-specific filler or a detail field.
// Bytes 2..=3 are the sequence number of the LAST REQUEST PROCESSED
// on the receiving connection — the client sets that when it
// enqueues the event, not the event builder.  The remaining 28 bytes
// carry per-event fields.
//
// This module produces raw 32-byte blocks; `Client::queue_event` fills
// in the sequence field at enqueue time.

pub const EV_KEY_PRESS:        u8 = 2;
pub const EV_KEY_RELEASE:      u8 = 3;
pub const EV_BUTTON_PRESS:     u8 = 4;
pub const EV_BUTTON_RELEASE:   u8 = 5;
pub const EV_MOTION_NOTIFY:    u8 = 6;
pub const EV_ENTER_NOTIFY:     u8 = 7;
pub const EV_LEAVE_NOTIFY:     u8 = 8;
pub const EV_FOCUS_IN:         u8 = 9;
pub const EV_FOCUS_OUT:        u8 = 10;
pub const EV_KEYMAP_NOTIFY:    u8 = 11;
pub const EV_EXPOSE:           u8 = 12;
pub const EV_GRAPHICS_EXPOSE:  u8 = 13;
pub const EV_NO_EXPOSE:        u8 = 14;
pub const EV_VISIBILITY_NOTIFY:u8 = 15;
pub const EV_CREATE_NOTIFY:    u8 = 16;
pub const EV_DESTROY_NOTIFY:   u8 = 17;
pub const EV_UNMAP_NOTIFY:     u8 = 18;
pub const EV_MAP_NOTIFY:       u8 = 19;
pub const EV_MAP_REQUEST:      u8 = 20;
pub const EV_REPARENT_NOTIFY:  u8 = 21;
pub const EV_CONFIGURE_NOTIFY: u8 = 22;
pub const EV_CONFIGURE_REQUEST:u8 = 23;
pub const EV_GRAVITY_NOTIFY:   u8 = 24;
pub const EV_RESIZE_REQUEST:   u8 = 25;
pub const EV_CIRCULATE_NOTIFY: u8 = 26;
pub const EV_CIRCULATE_REQUEST:u8 = 27;
pub const EV_PROPERTY_NOTIFY:  u8 = 28;
pub const EV_SELECTION_CLEAR:  u8 = 29;
pub const EV_SELECTION_REQUEST:u8 = 30;
pub const EV_SELECTION_NOTIFY: u8 = 31;
pub const EV_COLORMAP_NOTIFY:  u8 = 32;
pub const EV_CLIENT_MESSAGE:   u8 = 33;
pub const EV_MAPPING_NOTIFY:   u8 = 34;

fn base(code: u8) -> [u8; 32] {
    let mut ev = [0u8; 32];
    ev[0] = code;
    // bytes 2..=3 sequence number — filled in by Client::queue_event.
    ev
}

fn put_u16(ev: &mut [u8; 32], off: usize, v: u16) {
    let b = v.to_le_bytes();
    ev[off] = b[0]; ev[off+1] = b[1];
}

fn put_u32(ev: &mut [u8; 32], off: usize, v: u32) {
    let b = v.to_le_bytes();
    ev[off] = b[0]; ev[off+1] = b[1]; ev[off+2] = b[2]; ev[off+3] = b[3];
}

fn put_i16(ev: &mut [u8; 32], off: usize, v: i16) { put_u16(ev, off, v as u16); }

/// CreateNotify (16).
///
///     detail         1 byte unused
///     seq            2 bytes (filled at enqueue)
///     parent         4 bytes
///     window         4 bytes
///     x              2 bytes
///     y              2 bytes
///     width          2 bytes
///     height         2 bytes
///     border_width   2 bytes
///     override_redirect 1 byte
///     9 bytes unused
pub fn create_notify(
    parent: u32,
    window: u32,
    x: i16, y: i16,
    width: u16, height: u16,
    border_width: u16,
    override_redirect: bool,
) -> [u8; 32] {
    let mut ev = base(EV_CREATE_NOTIFY);
    put_u32(&mut ev, 4, parent);
    put_u32(&mut ev, 8, window);
    put_i16(&mut ev, 12, x);
    put_i16(&mut ev, 14, y);
    put_u16(&mut ev, 16, width);
    put_u16(&mut ev, 18, height);
    put_u16(&mut ev, 20, border_width);
    ev[22] = override_redirect as u8;
    ev
}

/// DestroyNotify (17).
///     event (window receiving)   4 bytes
///     window (the destroyed one) 4 bytes
pub fn destroy_notify(event_window: u32, window: u32) -> [u8; 32] {
    let mut ev = base(EV_DESTROY_NOTIFY);
    put_u32(&mut ev, 4, event_window);
    put_u32(&mut ev, 8, window);
    ev
}

/// UnmapNotify (18).
///     event          4 bytes
///     window         4 bytes
///     from_configure 1 byte
pub fn unmap_notify(event_window: u32, window: u32, from_configure: bool) -> [u8; 32] {
    let mut ev = base(EV_UNMAP_NOTIFY);
    put_u32(&mut ev, 4, event_window);
    put_u32(&mut ev, 8, window);
    ev[12] = from_configure as u8;
    ev
}

/// MapNotify (19).
///     event             4 bytes
///     window            4 bytes
///     override_redirect 1 byte
pub fn map_notify(event_window: u32, window: u32, override_redirect: bool) -> [u8; 32] {
    let mut ev = base(EV_MAP_NOTIFY);
    put_u32(&mut ev, 4, event_window);
    put_u32(&mut ev, 8, window);
    ev[12] = override_redirect as u8;
    ev
}

/// ConfigureNotify (22).
///     event        4 bytes
///     window       4 bytes
///     above_sibling 4 bytes (0 if topmost)
///     x            2
///     y            2
///     width        2
///     height       2
///     border_width 2
///     override_redirect 1
pub fn configure_notify(
    event_window: u32,
    window: u32,
    above_sibling: u32,
    x: i16, y: i16,
    width: u16, height: u16,
    border_width: u16,
    override_redirect: bool,
) -> [u8; 32] {
    let mut ev = base(EV_CONFIGURE_NOTIFY);
    put_u32(&mut ev, 4, event_window);
    put_u32(&mut ev, 8, window);
    put_u32(&mut ev, 12, above_sibling);
    put_i16(&mut ev, 16, x);
    put_i16(&mut ev, 18, y);
    put_u16(&mut ev, 20, width);
    put_u16(&mut ev, 22, height);
    put_u16(&mut ev, 24, border_width);
    ev[26] = override_redirect as u8;
    ev
}

/// Expose (12).
///     window 4 bytes
///     x      2
///     y      2
///     width  2
///     height 2
///     count  2  (0 = last in the series)
pub fn expose(window: u32, x: u16, y: u16, width: u16, height: u16, count: u16) -> [u8; 32] {
    let mut ev = base(EV_EXPOSE);
    put_u32(&mut ev, 4, window);
    put_u16(&mut ev, 8, x);
    put_u16(&mut ev, 10, y);
    put_u16(&mut ev, 12, width);
    put_u16(&mut ev, 14, height);
    put_u16(&mut ev, 16, count);
    ev
}

/// PropertyNotify (28).
///     window 4 bytes
///     atom   4 bytes
///     time   4 bytes (we always use 0 — "CurrentTime")
///     state  1 byte (0=NewValue, 1=Deleted)
pub fn property_notify(window: u32, atom: u32, deleted: bool) -> [u8; 32] {
    let mut ev = base(EV_PROPERTY_NOTIFY);
    put_u32(&mut ev, 4, window);
    put_u32(&mut ev, 8, atom);
    put_u32(&mut ev, 12, 0);  // time
    ev[16] = if deleted { 1 } else { 0 };
    ev
}

// ═════════════════════════════════════════════════════════════════════
// Input events (Phase 7)
// ═════════════════════════════════════════════════════════════════════
//
// KeyPress/KeyRelease/ButtonPress/ButtonRelease/MotionNotify all share
// a "device event" layout.  The detail byte (byte 1) is the keycode
// or button number; the rest of the body is the same.
//
//     code            byte 0
//     detail          byte 1  (keycode / button)
//     seq             bytes 2..4 (filled at enqueue)
//     time            bytes 4..8
//     root            bytes 8..12
//     event           bytes 12..16
//     child           bytes 16..20  (0 if none)
//     root_x          bytes 20..22
//     root_y          bytes 22..24
//     event_x         bytes 24..26
//     event_y         bytes 26..28
//     state           bytes 28..30  (KeyButMask)
//     same_screen     byte 30
//     byte 31         unused

#[allow(clippy::too_many_arguments)]
fn device_event(
    code: u8,
    detail: u8,
    time: u32,
    root: u32,
    event: u32,
    child: u32,
    root_x: i16, root_y: i16,
    event_x: i16, event_y: i16,
    state: u16,
    same_screen: bool,
) -> [u8; 32] {
    let mut ev = base(code);
    ev[1] = detail;
    put_u32(&mut ev, 4, time);
    put_u32(&mut ev, 8, root);
    put_u32(&mut ev, 12, event);
    put_u32(&mut ev, 16, child);
    put_i16(&mut ev, 20, root_x);
    put_i16(&mut ev, 22, root_y);
    put_i16(&mut ev, 24, event_x);
    put_i16(&mut ev, 26, event_y);
    put_u16(&mut ev, 28, state);
    ev[30] = same_screen as u8;
    ev
}

pub fn key_press(
    keycode: u8, time: u32, root: u32, event: u32, child: u32,
    root_xy: (i16, i16), event_xy: (i16, i16), state: u16,
) -> [u8; 32] {
    device_event(EV_KEY_PRESS, keycode, time, root, event, child,
                 root_xy.0, root_xy.1, event_xy.0, event_xy.1, state, true)
}

pub fn key_release(
    keycode: u8, time: u32, root: u32, event: u32, child: u32,
    root_xy: (i16, i16), event_xy: (i16, i16), state: u16,
) -> [u8; 32] {
    device_event(EV_KEY_RELEASE, keycode, time, root, event, child,
                 root_xy.0, root_xy.1, event_xy.0, event_xy.1, state, true)
}

pub fn button_press(
    button: u8, time: u32, root: u32, event: u32, child: u32,
    root_xy: (i16, i16), event_xy: (i16, i16), state: u16,
) -> [u8; 32] {
    device_event(EV_BUTTON_PRESS, button, time, root, event, child,
                 root_xy.0, root_xy.1, event_xy.0, event_xy.1, state, true)
}

pub fn button_release(
    button: u8, time: u32, root: u32, event: u32, child: u32,
    root_xy: (i16, i16), event_xy: (i16, i16), state: u16,
) -> [u8; 32] {
    device_event(EV_BUTTON_RELEASE, button, time, root, event, child,
                 root_xy.0, root_xy.1, event_xy.0, event_xy.1, state, true)
}

pub fn motion_notify(
    detail: u8, time: u32, root: u32, event: u32, child: u32,
    root_xy: (i16, i16), event_xy: (i16, i16), state: u16,
) -> [u8; 32] {
    device_event(EV_MOTION_NOTIFY, detail, time, root, event, child,
                 root_xy.0, root_xy.1, event_xy.0, event_xy.1, state, true)
}

/// EnterNotify / LeaveNotify share a layout similar to device events
/// but include `mode` and `focus` fields instead of `same_screen`
/// alone.
///
///     detail          byte 1
///     time            4..8
///     root            8..12
///     event           12..16
///     child           16..20
///     root_x          20..22
///     root_y          22..24
///     event_x         24..26
///     event_y         26..28
///     state           28..30
///     mode            byte 30
///     same_screen_focus byte 31
#[allow(clippy::too_many_arguments)]
pub fn enter_notify(
    detail: u8, time: u32, root: u32, event: u32, child: u32,
    root_xy: (i16, i16), event_xy: (i16, i16), state: u16,
    mode: u8, focus: bool,
) -> [u8; 32] {
    let mut ev = base(EV_ENTER_NOTIFY);
    ev[1] = detail;
    put_u32(&mut ev, 4, time);
    put_u32(&mut ev, 8, root);
    put_u32(&mut ev, 12, event);
    put_u32(&mut ev, 16, child);
    put_i16(&mut ev, 20, root_xy.0);
    put_i16(&mut ev, 22, root_xy.1);
    put_i16(&mut ev, 24, event_xy.0);
    put_i16(&mut ev, 26, event_xy.1);
    put_u16(&mut ev, 28, state);
    ev[30] = mode;
    ev[31] = (focus as u8) | 0b10; // same_screen = 1
    ev
}

#[allow(clippy::too_many_arguments)]
pub fn leave_notify(
    detail: u8, time: u32, root: u32, event: u32, child: u32,
    root_xy: (i16, i16), event_xy: (i16, i16), state: u16,
    mode: u8, focus: bool,
) -> [u8; 32] {
    let mut ev = enter_notify(
        detail, time, root, event, child, root_xy, event_xy,
        state, mode, focus,
    );
    ev[0] = EV_LEAVE_NOTIFY;
    ev
}

/// FocusIn / FocusOut (9 / 10).
///     detail byte 1 (NotifyAncestor=0, NotifyVirtual=1, …)
///     event  bytes 4..8
///     mode   byte 8 (NotifyNormal=0, NotifyGrab=1, NotifyUngrab=2)
pub fn focus_in(detail: u8, event: u32, mode: u8) -> [u8; 32] {
    let mut ev = base(EV_FOCUS_IN);
    ev[1] = detail;
    put_u32(&mut ev, 4, event);
    ev[8] = mode;
    ev
}

pub fn focus_out(detail: u8, event: u32, mode: u8) -> [u8; 32] {
    let mut ev = focus_in(detail, event, mode);
    ev[0] = EV_FOCUS_OUT;
    ev
}

// ═════════════════════════════════════════════════════════════════════
// Window-manager events (Phase 8)
// ═════════════════════════════════════════════════════════════════════

/// ReparentNotify (21).
///     event      4..8  (the window receiving the event)
///     window     8..12 (the reparented window)
///     parent     12..16 (new parent)
///     x          16..18
///     y          18..20
///     override_redirect byte 20
pub fn reparent_notify(
    event_window: u32, window: u32, parent: u32,
    x: i16, y: i16, override_redirect: bool,
) -> [u8; 32] {
    let mut ev = base(EV_REPARENT_NOTIFY);
    put_u32(&mut ev, 4, event_window);
    put_u32(&mut ev, 8, window);
    put_u32(&mut ev, 12, parent);
    put_i16(&mut ev, 16, x);
    put_i16(&mut ev, 18, y);
    ev[20] = override_redirect as u8;
    ev
}

/// MapRequest (20).
///     parent 4..8
///     window 8..12
pub fn map_request(parent: u32, window: u32) -> [u8; 32] {
    let mut ev = base(EV_MAP_REQUEST);
    put_u32(&mut ev, 4, parent);
    put_u32(&mut ev, 8, window);
    ev
}

/// ConfigureRequest (23).
///     stack_mode byte 1
///     parent  4..8
///     window  8..12
///     sibling 12..16
///     x       16..18  y 18..20
///     width   20..22  height 22..24
///     border_width 24..26
///     value_mask 26..28
pub fn configure_request(
    stack_mode: u8,
    parent: u32, window: u32, sibling: u32,
    x: i16, y: i16, width: u16, height: u16, border_width: u16,
    value_mask: u16,
) -> [u8; 32] {
    let mut ev = base(EV_CONFIGURE_REQUEST);
    ev[1] = stack_mode;
    put_u32(&mut ev, 4, parent);
    put_u32(&mut ev, 8, window);
    put_u32(&mut ev, 12, sibling);
    put_i16(&mut ev, 16, x);
    put_i16(&mut ev, 18, y);
    put_u16(&mut ev, 20, width);
    put_u16(&mut ev, 22, height);
    put_u16(&mut ev, 24, border_width);
    put_u16(&mut ev, 26, value_mask);
    ev
}

/// CirculateRequest (27) / CirculateNotify (26).
///     event / parent 4..8
///     window         8..12
///     place byte 16  (0=Top, 1=Bottom)
pub fn circulate_request(parent: u32, window: u32, place: u8) -> [u8; 32] {
    let mut ev = base(EV_CIRCULATE_REQUEST);
    put_u32(&mut ev, 4, parent);
    put_u32(&mut ev, 8, window);
    ev[16] = place;
    ev
}

pub fn circulate_notify(event_window: u32, window: u32, place: u8) -> [u8; 32] {
    let mut ev = base(EV_CIRCULATE_NOTIFY);
    put_u32(&mut ev, 4, event_window);
    put_u32(&mut ev, 8, window);
    ev[16] = place;
    ev
}
