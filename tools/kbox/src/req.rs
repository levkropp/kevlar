// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// X11 request builders for the eight opcodes kbox needs to satisfy
// `xprop -root _NET_SUPPORTING_WM_CHECK`.  Each function appends
// the request bytes to a `Vec<u8>` write buffer; the caller flushes.
//
// We do NOT do BIG-REQUESTS; every request fits in 65535 words and
// the length field stays in the 16-bit header.

#![allow(dead_code)]

use std::io::{self, Write};

use crate::conn::X11Conn;
use crate::wire::{align4, put_pad, put_u16, put_u32, put_u8};
use crate::{kbox_log, log, req};

// ── Opcodes (X11 core) ─────────────────────────────────────────────

pub const OP_CREATE_WINDOW:        u8 = 1;
pub const OP_CHANGE_WINDOW_ATTRS:  u8 = 2;
pub const OP_MAP_WINDOW:           u8 = 8;
pub const OP_INTERN_ATOM:          u8 = 16;
pub const OP_CHANGE_PROPERTY:      u8 = 18;
pub const OP_GET_PROPERTY:         u8 = 20;
pub const OP_LIST_PROPERTIES:      u8 = 21;
pub const OP_SET_SELECTION_OWNER:  u8 = 22;
pub const OP_GET_SELECTION_OWNER:  u8 = 23;
pub const OP_GRAB_BUTTON:          u8 = 28;
pub const OP_GRAB_KEY:             u8 = 33;
pub const OP_QUERY_TREE:           u8 = 15;
pub const OP_GET_GEOMETRY:         u8 = 14;
pub const OP_GET_WINDOW_ATTRS:     u8 = 3;
pub const OP_SET_INPUT_FOCUS:      u8 = 42;
pub const OP_GET_KEYBOARD_MAPPING: u8 = 101;
pub const OP_QUERY_EXTENSION:      u8 = 98;

// ── Predefined atoms (XA_*) ────────────────────────────────────────

pub const XA_PRIMARY:    u32 = 1;
pub const XA_STRING:     u32 = 31;
pub const XA_WINDOW:     u32 = 33;
pub const XA_WM_NAME:    u32 = 39;
pub const XA_WM_CLASS:   u32 = 67;

// ── Window classes ─────────────────────────────────────────────────

pub const WIN_INPUT_OUTPUT: u16 = 1;
pub const WIN_INPUT_ONLY:   u16 = 2;

// ── ChangeProperty modes ───────────────────────────────────────────

pub const PROP_REPLACE: u8 = 0;
pub const PROP_PREPEND: u8 = 1;
pub const PROP_APPEND:  u8 = 2;

// ── CurrentTime ────────────────────────────────────────────────────

pub const CURRENT_TIME: u32 = 0;

// ───────────────────────────────────────────────────────────────────
// Each request returns the sequence number it was sent at, for
// matching replies.  `conn.seq_next` is bumped whether or not the
// request carries a reply — X11 numbers ALL requests, not just the
// ones with replies.
// ───────────────────────────────────────────────────────────────────

/// `InternAtom`: returns the atom id in the reply (4 bytes at offset 8).
pub fn intern_atom(out: &mut Vec<u8>, conn: &mut X11Conn, name: &str, only_if_exists: bool) -> u16 {
    let name_bytes = name.as_bytes();
    let payload_len = name_bytes.len();
    let total = 4 + 4 + payload_len; // header(4) + len/pad(4) + name
    let words = (total + 3) / 4;

    put_u8(out, OP_INTERN_ATOM);
    put_u8(out, only_if_exists as u8);
    put_u16(out, words as u16);
    put_u16(out, payload_len as u16);
    put_u16(out, 0); // pad
    out.extend_from_slice(name_bytes);
    align4(out);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} InternAtom only_if_exists={} name={:?}", seq, only_if_exists, name);
    seq
}

/// `CreateWindow` for a 1x1 InputOnly child of `parent`.  Used for the
/// EWMH check window.  `value-mask = 0` means "no attribute overrides".
pub fn create_window_input_only(out: &mut Vec<u8>, conn: &mut X11Conn,
                                parent: u32, root_visual: u32) -> (u16, u32) {
    let wid = conn.alloc_xid();

    // Header word + 7 fixed words = 32 bytes (length=8 words), no value-mask data.
    put_u8(out, OP_CREATE_WINDOW);
    put_u8(out, 0);             // depth = CopyFromParent (0)
    put_u16(out, 8);            // length = 8 words
    put_u32(out, wid);          // wid
    put_u32(out, parent);       // parent
    put_u16(out, 0);            // x
    put_u16(out, 0);            // y
    put_u16(out, 1);            // width
    put_u16(out, 1);            // height
    put_u16(out, 0);            // border-width
    put_u16(out, WIN_INPUT_ONLY); // class
    put_u32(out, root_visual);  // visual = CopyFromParent's visual
    put_u32(out, 0);            // value-mask = 0

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} CreateWindow wid=0x{:x} parent=0x{:x} class=InputOnly visual=0x{:x}",
         seq, wid, parent, root_visual);
    (seq, wid)
}

/// `SetSelectionOwner`: claim ownership of a selection atom for `owner_window`.
pub fn set_selection_owner(out: &mut Vec<u8>, conn: &mut X11Conn,
                           owner_window: u32, selection_atom: u32) -> u16 {
    put_u8(out, OP_SET_SELECTION_OWNER);
    put_u8(out, 0);
    put_u16(out, 4);                // length = 4 words
    put_u32(out, owner_window);
    put_u32(out, selection_atom);
    put_u32(out, CURRENT_TIME);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} SetSelectionOwner owner=0x{:x} selection=0x{:x} time=CurrentTime",
         seq, owner_window, selection_atom);
    seq
}

/// `ChangeProperty` with `format=32`, used for a single WINDOW value.
pub fn change_property_window(out: &mut Vec<u8>, conn: &mut X11Conn,
                              window: u32, property: u32, value_window: u32) -> u16 {
    let data_bytes = 4; // one u32
    let total = 24 + data_bytes;
    let words = total / 4;

    put_u8(out, OP_CHANGE_PROPERTY);
    put_u8(out, PROP_REPLACE);
    put_u16(out, words as u16);
    put_u32(out, window);
    put_u32(out, property);
    put_u32(out, XA_WINDOW);   // type = WINDOW
    put_u8(out, 32);            // format
    put_pad(out, 3);            // pad
    put_u32(out, 1);            // length-of-data (in format units, here 1 × u32)
    put_u32(out, value_window); // the actual data

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} ChangeProperty win=0x{:x} property=0x{:x} type=WINDOW fmt=32 len=1 value=0x{:x}",
         seq, window, property, value_window);
    seq
}

/// `ChangeProperty` with `format=8`, used for a UTF8_STRING value.
pub fn change_property_string(out: &mut Vec<u8>, conn: &mut X11Conn,
                              window: u32, property: u32, type_atom: u32,
                              data: &[u8]) -> u16 {
    let data_len = data.len();
    let data_padded = (data_len + 3) & !3;
    let total = 24 + data_padded;
    let words = total / 4;

    put_u8(out, OP_CHANGE_PROPERTY);
    put_u8(out, PROP_REPLACE);
    put_u16(out, words as u16);
    put_u32(out, window);
    put_u32(out, property);
    put_u32(out, type_atom);
    put_u8(out, 8);             // format
    put_pad(out, 3);            // pad
    put_u32(out, data_len as u32);
    out.extend_from_slice(data);
    align4(out);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} ChangeProperty win=0x{:x} property=0x{:x} type=0x{:x} fmt=8 len={} bytes={:?}",
         seq, window, property, type_atom, data_len,
         core::str::from_utf8(data).unwrap_or("<non-utf8>"));
    seq
}

/// Flush the accumulated write buffer to the socket.  After return,
/// the buffer is empty.
pub fn flush(out: &mut Vec<u8>, conn: &mut X11Conn) -> io::Result<()> {
    if out.is_empty() { return Ok(()); }
    log::hex_dump("send-flush", out);
    conn.sock.write_all(out)?;
    conn.sock.flush()?;
    out.clear();
    Ok(())
}

// ── Event mask bits (CW_EVENT_MASK) ─────────────────────────────────

pub const EVT_KEY_PRESS:           u32 = 1 << 0;
pub const EVT_KEY_RELEASE:         u32 = 1 << 1;
pub const EVT_BUTTON_PRESS:        u32 = 1 << 2;
pub const EVT_BUTTON_RELEASE:      u32 = 1 << 3;
pub const EVT_ENTER_WINDOW:        u32 = 1 << 4;
pub const EVT_LEAVE_WINDOW:        u32 = 1 << 5;
pub const EVT_POINTER_MOTION:      u32 = 1 << 6;
pub const EVT_EXPOSURE:            u32 = 1 << 15;
pub const EVT_VISIBILITY_CHANGE:   u32 = 1 << 16;
pub const EVT_STRUCTURE_NOTIFY:    u32 = 1 << 17;
pub const EVT_RESIZE_REDIRECT:     u32 = 1 << 18;
pub const EVT_SUBSTRUCTURE_NOTIFY: u32 = 1 << 19;
pub const EVT_SUBSTRUCTURE_REDIRECT:u32= 1 << 20;
pub const EVT_FOCUS_CHANGE:        u32 = 1 << 21;
pub const EVT_PROPERTY_CHANGE:     u32 = 1 << 22;

// ── ChangeWindowAttributes value-mask bits ──────────────────────────

pub const CW_BACK_PIXMAP:          u32 = 1 << 0;
pub const CW_BACK_PIXEL:           u32 = 1 << 1;
pub const CW_BORDER_PIXMAP:        u32 = 1 << 2;
pub const CW_BORDER_PIXEL:         u32 = 1 << 3;
pub const CW_BIT_GRAVITY:          u32 = 1 << 4;
pub const CW_WIN_GRAVITY:          u32 = 1 << 5;
pub const CW_BACKING_STORE:        u32 = 1 << 6;
pub const CW_BACKING_PLANES:       u32 = 1 << 7;
pub const CW_BACKING_PIXEL:        u32 = 1 << 8;
pub const CW_OVERRIDE_REDIRECT:    u32 = 1 << 9;
pub const CW_SAVE_UNDER:           u32 = 1 << 10;
pub const CW_EVENT_MASK:           u32 = 1 << 11;
pub const CW_DONT_PROPAGATE:       u32 = 1 << 12;
pub const CW_COLORMAP:             u32 = 1 << 13;
pub const CW_CURSOR:               u32 = 1 << 14;

// ── SendEvent + ClientMessage (phase 17) ──────────────────────────
pub const OP_SEND_EVENT: u8 = 25;

/// `SendEvent`: synthesise an event and have the server deliver it
/// to clients selecting `event_mask` on `destination`.
///
/// `event_bytes` is exactly 32 bytes — the event payload as it
/// would appear on the wire when the server sends an event.  For a
/// ClientMessage event:
///   byte 0: code = 33 (ClientMessage)
///   byte 1: format (8, 16, or 32)
///   bytes 2-3: sequence (overwritten by server, but must be present)
///   bytes 4-7: window
///   bytes 8-11: message-type atom
///   bytes 12-31: 20 bytes of data (interpretation depends on format)
///
/// `propagate=false` is the typical WM usage (deliver only to
/// selecting clients on `destination`, no walk up the tree).
pub fn send_event(out: &mut Vec<u8>, conn: &mut X11Conn,
                  propagate: bool, destination: u32,
                  event_mask: u32, event_bytes: &[u8; 32]) -> u16 {
    // Total = 4 (header) + 4 (destination) + 4 (event_mask) + 32 (event)
    //       = 44 bytes = 11 words.
    put_u8(out, OP_SEND_EVENT);
    put_u8(out, propagate as u8);
    put_u16(out, 11);
    put_u32(out, destination);
    put_u32(out, event_mask);
    out.extend_from_slice(event_bytes);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} SendEvent dest=0x{:x} mask=0x{:x} event_code={}",
         seq, destination, event_mask, event_bytes[0]);
    seq
}

/// Build a 32-byte ClientMessage event payload.
pub fn build_client_message(format: u8, window: u32, message_type: u32,
                            data: &[u8]) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[0] = 33; // ClientMessage event code
    e[1] = format;
    // bytes 2-3: sequence — server fills, leave 0
    e[4..8].copy_from_slice(&window.to_le_bytes());
    e[8..12].copy_from_slice(&message_type.to_le_bytes());
    let n = data.len().min(20);
    e[12..12 + n].copy_from_slice(&data[..n]);
    e
}

/// `ChangeProperty` with `mode=Append`, `format=8`, arbitrary type atom.
/// Used by phase 17 to mimic openbox's WM_CLASS-style append-string
/// pattern.
pub fn change_property_append_string(out: &mut Vec<u8>, conn: &mut X11Conn,
                                     window: u32, property: u32,
                                     type_atom: u32, data: &[u8]) -> u16 {
    let data_len = data.len();
    let data_padded = (data_len + 3) & !3;
    let total = 24 + data_padded;
    let words = total / 4;
    put_u8(out, OP_CHANGE_PROPERTY);
    put_u8(out, PROP_APPEND);
    put_u16(out, words as u16);
    put_u32(out, window);
    put_u32(out, property);
    put_u32(out, type_atom);
    put_u8(out, 8);
    put_pad(out, 3);
    put_u32(out, data_len as u32);
    out.extend_from_slice(data);
    align4(out);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} ChangeProperty(Append) win=0x{:x} property=0x{:x} type=0x{:x} fmt=8 len={}",
         seq, window, property, type_atom, data_len);
    seq
}

// ── Core opcodes used in phase 12 ─────────────────────────────────
pub const OP_MAP_REQUEST_RAW:    u8 = 8;   // alias for clarity (MapWindow)
pub const OP_CONFIGURE_WINDOW:   u8 = 12;
pub const OP_CREATE_GC:          u8 = 55;
pub const OP_FREE_GC:            u8 = 60;
pub const OP_ALLOC_NAMED_COLOR:  u8 = 85;
pub const OP_CREATE_PIXMAP:      u8 = 53;
pub const OP_FREE_PIXMAP:        u8 = 54;

/// `MapWindow`: make a window viewable.  No reply.
pub fn map_window(out: &mut Vec<u8>, conn: &mut X11Conn, window: u32) -> u16 {
    put_u8(out, OP_MAP_WINDOW);
    put_u8(out, 0);
    put_u16(out, 2);
    put_u32(out, window);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} MapWindow win=0x{:x}", seq, window);
    seq
}

/// `ConfigureWindow` with a value-mask + values.  Caller passes the
/// (mask, values) pair.  Bit positions in mask: 0=x, 1=y, 2=w, 3=h,
/// 4=border, 5=sibling, 6=stack_mode.
pub fn configure_window(out: &mut Vec<u8>, conn: &mut X11Conn,
                        window: u32, value_mask: u16, values: &[u32]) -> u16 {
    let total = 12 + values.len() * 4;
    let words = total / 4;
    put_u8(out, OP_CONFIGURE_WINDOW);
    put_u8(out, 0);
    put_u16(out, words as u16);
    put_u32(out, window);
    put_u16(out, value_mask);
    put_u16(out, 0);
    for v in values { put_u32(out, *v); }
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} ConfigureWindow win=0x{:x} mask=0x{:x} nvalues={}",
         seq, window, value_mask, values.len());
    seq
}

/// `CreateGC` with no override values (all defaults).  4-word request.
pub fn create_gc_default(out: &mut Vec<u8>, conn: &mut X11Conn,
                         drawable: u32) -> (u16, u32) {
    let cid = conn.alloc_xid();
    put_u8(out, OP_CREATE_GC);
    put_u8(out, 0);
    put_u16(out, 4);
    put_u32(out, cid);
    put_u32(out, drawable);
    put_u32(out, 0); // value-mask = none
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} CreateGC cid=0x{:x} drawable=0x{:x}", seq, cid, drawable);
    (seq, cid)
}

/// `FreeGC`: release a graphics context.
pub fn free_gc(out: &mut Vec<u8>, conn: &mut X11Conn, gc: u32) -> u16 {
    put_u8(out, OP_FREE_GC);
    put_u8(out, 0);
    put_u16(out, 2);
    put_u32(out, gc);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} FreeGC gc=0x{:x}", seq, gc);
    seq
}

/// `AllocNamedColor`: ask the server to look up a colour name and
/// return its allocated pixel + RGB triple.
pub fn alloc_named_color(out: &mut Vec<u8>, conn: &mut X11Conn,
                         cmap: u32, name: &str) -> u16 {
    let nbytes = name.len();
    let total = 12 + ((nbytes + 3) & !3);
    let words = total / 4;
    put_u8(out, OP_ALLOC_NAMED_COLOR);
    put_u8(out, 0);
    put_u16(out, words as u16);
    put_u32(out, cmap);
    put_u16(out, nbytes as u16);
    put_u16(out, 0);
    out.extend_from_slice(name.as_bytes());
    align4(out);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} AllocNamedColor cmap=0x{:x} name={:?}", seq, cmap, name);
    seq
}

/// `CreatePixmap` with the given depth, size, and parent drawable.
pub fn create_pixmap(out: &mut Vec<u8>, conn: &mut X11Conn,
                     depth: u8, drawable: u32, w: u16, h: u16) -> (u16, u32) {
    let pid = conn.alloc_xid();
    put_u8(out, OP_CREATE_PIXMAP);
    put_u8(out, depth);
    put_u16(out, 4);
    put_u32(out, pid);
    put_u32(out, drawable);
    put_u16(out, w);
    put_u16(out, h);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} CreatePixmap pid=0x{:x} drawable=0x{:x} {}x{}@{}",
         seq, pid, drawable, w, h, depth);
    (seq, pid)
}

/// `FreePixmap`.
pub fn free_pixmap(out: &mut Vec<u8>, conn: &mut X11Conn, pixmap: u32) -> u16 {
    put_u8(out, OP_FREE_PIXMAP);
    put_u8(out, 0);
    put_u16(out, 2);
    put_u32(out, pixmap);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} FreePixmap pix=0x{:x}", seq, pixmap);
    seq
}

// ── MIT-SHM extension minor opcodes ───────────────────────────────
pub const SHM_QUERY_VERSION:  u8 = 0;
pub const SHM_ATTACH:         u8 = 1;
pub const SHM_DETACH:         u8 = 2;

/// `MitShmQueryVersion`: returns server-supported SHM version + uid/gid/pid_supported.
pub fn shm_query_version(out: &mut Vec<u8>, conn: &mut X11Conn,
                         shm_major_opcode: u8) -> u16 {
    put_u8(out, shm_major_opcode);
    put_u8(out, SHM_QUERY_VERSION);
    put_u16(out, 1);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} MitShmQueryVersion", seq);
    seq
}

/// `MitShmAttach(shmseg, shmid, read_only)` — attach a SHM segment
/// previously created via shmget(2) on the client side.  No reply.
/// On error, the server returns an X11 Error frame asynchronously.
pub fn shm_attach(out: &mut Vec<u8>, conn: &mut X11Conn,
                  shm_major_opcode: u8,
                  shmseg: u32, shmid: u32, read_only: bool) -> u16 {
    put_u8(out, shm_major_opcode);
    put_u8(out, SHM_ATTACH);
    put_u16(out, 4);
    put_u32(out, shmseg);
    put_u32(out, shmid);
    put_u8(out, read_only as u8);
    put_u8(out, 0); put_u8(out, 0); put_u8(out, 0);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} MitShmAttach shmseg=0x{:x} shmid={} read_only={}",
         seq, shmseg, shmid, read_only);
    seq
}

// ── XKEYBOARD extension (XKB) minor opcodes ──────────────────────
pub const XKB_USE_EXTENSION:  u8 = 0;
pub const XKB_SELECT_EVENTS:  u8 = 1;
pub const XKB_GET_STATE:      u8 = 4;
pub const XKB_GET_CONTROLS:   u8 = 6;
pub const XKB_GET_MAP:        u8 = 8;
pub const XKB_GET_NAMES:      u8 = 17;

/// `XkbUseExtension`: negotiate XKB protocol version.
/// Format: (xkb_major, 0, length=2, wanted_major u16, wanted_minor u16)
pub fn xkb_use_extension(out: &mut Vec<u8>, conn: &mut X11Conn,
                         xkb_major_opcode: u8,
                         wanted_major: u16, wanted_minor: u16) -> u16 {
    put_u8(out, xkb_major_opcode);
    put_u8(out, XKB_USE_EXTENSION);
    put_u16(out, 2);
    put_u16(out, wanted_major);
    put_u16(out, wanted_minor);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} XkbUseExtension wanted={}.{}",
         seq, wanted_major, wanted_minor);
    seq
}

/// `XkbGetState`: read the current keyboard state for a device.
/// `device_spec=0x100` = UseCoreKbd.  Length = 2 words.
pub fn xkb_get_state(out: &mut Vec<u8>, conn: &mut X11Conn,
                     xkb_major_opcode: u8, device_spec: u16) -> u16 {
    put_u8(out, xkb_major_opcode);
    put_u8(out, XKB_GET_STATE);
    put_u16(out, 2);
    put_u16(out, device_spec);
    put_u16(out, 0); // pad

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} XkbGetState device=0x{:x}", seq, device_spec);
    seq
}

/// `XkbGetControls`: get control flags for a device.
pub fn xkb_get_controls(out: &mut Vec<u8>, conn: &mut X11Conn,
                        xkb_major_opcode: u8, device_spec: u16) -> u16 {
    put_u8(out, xkb_major_opcode);
    put_u8(out, XKB_GET_CONTROLS);
    put_u16(out, 2);
    put_u16(out, device_spec);
    put_u16(out, 0);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} XkbGetControls device=0x{:x}", seq, device_spec);
    seq
}

/// `XkbGetNames`: get keyboard component names.  `which` is a bitmask
/// of which name groups to return.  Use 0xFFFFFFFF for "everything".
pub fn xkb_get_names(out: &mut Vec<u8>, conn: &mut X11Conn,
                     xkb_major_opcode: u8, device_spec: u16, which: u32) -> u16 {
    put_u8(out, xkb_major_opcode);
    put_u8(out, XKB_GET_NAMES);
    put_u16(out, 3);
    put_u16(out, device_spec);
    put_u16(out, 0);
    put_u32(out, which);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} XkbGetNames device=0x{:x} which=0x{:x}",
         seq, device_spec, which);
    seq
}

/// `QueryExtension`: ask the server whether `name` is supported.
/// Reply: u8 present, u8 major_opcode, u8 first_event, u8 first_error.
pub fn query_extension(out: &mut Vec<u8>, conn: &mut X11Conn, name: &str) -> u16 {
    let bytes = name.as_bytes();
    let nbytes = bytes.len();
    let total = 4 + 4 + nbytes; // header(4) + len/pad(4) + name (padded)
    let words = (total + 3) / 4;

    put_u8(out, OP_QUERY_EXTENSION);
    put_u8(out, 0);
    put_u16(out, words as u16);
    put_u16(out, nbytes as u16);
    put_u16(out, 0);
    out.extend_from_slice(bytes);
    align4(out);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} QueryExtension name={:?}", seq, name);
    seq
}

/// `GetKeyboardMapping`: returns the keysyms-per-keycode table.
/// Reply: 32-byte header + (count * keysyms_per_keycode * 4) bytes.
pub fn get_keyboard_mapping(out: &mut Vec<u8>, conn: &mut X11Conn,
                            first_keycode: u8, count: u8) -> u16 {
    put_u8(out, OP_GET_KEYBOARD_MAPPING);
    put_u8(out, 0);
    put_u16(out, 2);
    put_u8(out, first_keycode);
    put_u8(out, count);
    put_u16(out, 0);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} GetKeyboardMapping first_keycode={} count={}",
         seq, first_keycode, count);
    seq
}

/// `GrabKey`: passive grab on a key combo for a window.
/// `modifiers` = bitmask (1=Shift, 2=Lock/Caps, 4=Control, 8=Mod1/Alt, ...).
/// `modifiers = 0x8000` means AnyModifier (grab regardless of mods).
pub fn grab_key(out: &mut Vec<u8>, conn: &mut X11Conn,
                owner_events: bool, grab_window: u32,
                modifiers: u16, key: u8,
                pointer_mode: u8, keyboard_mode: u8) -> u16 {
    put_u8(out, OP_GRAB_KEY);
    put_u8(out, owner_events as u8);
    put_u16(out, 4);                // length = 4 words
    put_u32(out, grab_window);
    put_u16(out, modifiers);
    put_u8(out, key);
    put_u8(out, pointer_mode);
    put_u8(out, keyboard_mode);
    put_u8(out, 0); put_u8(out, 0); put_u8(out, 0);   // pad
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} GrabKey win=0x{:x} mods=0x{:x} key={}",
         seq, grab_window, modifiers, key);
    seq
}

/// `GrabButton`: passive grab on a button combo for a window.
/// `button = 0` = AnyButton, `modifiers = 0x8000` = AnyModifier.
pub fn grab_button(out: &mut Vec<u8>, conn: &mut X11Conn,
                   owner_events: bool, grab_window: u32,
                   event_mask: u16, pointer_mode: u8, keyboard_mode: u8,
                   confine_to: u32, cursor: u32,
                   button: u8, modifiers: u16) -> u16 {
    put_u8(out, OP_GRAB_BUTTON);
    put_u8(out, owner_events as u8);
    put_u16(out, 6);                // length = 6 words
    put_u32(out, grab_window);
    put_u16(out, event_mask);
    put_u8(out, pointer_mode);
    put_u8(out, keyboard_mode);
    put_u32(out, confine_to);
    put_u32(out, cursor);
    put_u8(out, button);
    put_u8(out, 0);                 // pad
    put_u16(out, modifiers);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} GrabButton win=0x{:x} button={} mods=0x{:x}",
         seq, grab_window, button, modifiers);
    seq
}

/// `SetInputFocus`: change which window has the keyboard focus.
/// `revert_to`: 0=None, 1=PointerRoot, 2=Parent.
pub fn set_input_focus(out: &mut Vec<u8>, conn: &mut X11Conn,
                       revert_to: u8, focus: u32, time: u32) -> u16 {
    put_u8(out, OP_SET_INPUT_FOCUS);
    put_u8(out, revert_to);
    put_u16(out, 3);                // length = 3 words
    put_u32(out, focus);
    put_u32(out, time);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} SetInputFocus revert_to={} focus=0x{:x} time={}",
         seq, revert_to, focus, time);
    seq
}

/// `ListProperties`: returns the list of atom IDs of properties set on `window`.
/// Reply: 32-byte header + 4*natoms bytes (each atom is u32).
pub fn list_properties(out: &mut Vec<u8>, conn: &mut X11Conn, window: u32) -> u16 {
    put_u8(out, OP_LIST_PROPERTIES);
    put_u8(out, 0);
    put_u16(out, 2);
    put_u32(out, window);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} ListProperties win=0x{:x}", seq, window);
    seq
}

/// `GetProperty`: read a property from a window.
/// Reply: 32-byte header + (length * format/8) bytes padded to 4.
/// Use `delete=false`, `type=AnyPropertyType (0)`, offset=0, length=1024
/// (in 4-byte units = 4 KiB max — plenty for typical EWMH props).
pub fn get_property(out: &mut Vec<u8>, conn: &mut X11Conn,
                    window: u32, property: u32) -> u16 {
    put_u8(out, OP_GET_PROPERTY);
    put_u8(out, 0);              // delete = false
    put_u16(out, 6);             // length = 6 words
    put_u32(out, window);
    put_u32(out, property);
    put_u32(out, 0);             // type = AnyPropertyType
    put_u32(out, 0);             // long-offset = 0
    put_u32(out, 1024);          // long-length = 1024 (in 4-byte units)
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} GetProperty win=0x{:x} property=0x{:x}",
         seq, window, property);
    seq
}

/// `QueryTree`: returns the root, parent, and children of the given window.
/// Reply is variable-length (1 + 8 + 4*nchildren bytes after the 32-byte header).
pub fn query_tree(out: &mut Vec<u8>, conn: &mut X11Conn, window: u32) -> u16 {
    put_u8(out, OP_QUERY_TREE);
    put_u8(out, 0);
    put_u16(out, 2);            // length = 2 words
    put_u32(out, window);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} QueryTree win=0x{:x}", seq, window);
    seq
}

/// `GetWindowAttributes`: returns 44 bytes of window attribute data.
pub fn get_window_attributes(out: &mut Vec<u8>, conn: &mut X11Conn, window: u32) -> u16 {
    put_u8(out, OP_GET_WINDOW_ATTRS);
    put_u8(out, 0);
    put_u16(out, 2);
    put_u32(out, window);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} GetWindowAttributes win=0x{:x}", seq, window);
    seq
}

/// `GetGeometry`: x, y, width, height, border-width, depth, root.
pub fn get_geometry(out: &mut Vec<u8>, conn: &mut X11Conn, drawable: u32) -> u16 {
    put_u8(out, OP_GET_GEOMETRY);
    put_u8(out, 0);
    put_u16(out, 2);
    put_u32(out, drawable);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} GetGeometry drawable=0x{:x}", seq, drawable);
    seq
}

pub const OP_OPEN_FONT:           u8 = 45;
pub const OP_CREATE_GLYPH_CURSOR: u8 = 94;

/// `OpenFont`: open a server-side font under the given XID.
/// Phase 19 uses this with name "cursor" to access the standard
/// X11 cursor glyph font.
pub fn open_font(out: &mut Vec<u8>, conn: &mut X11Conn, name: &str) -> (u16, u32) {
    let fid = conn.alloc_xid();
    let name_bytes = name.as_bytes();
    let payload_len = name_bytes.len();
    let total = 4 + 4 + 4 + payload_len; // header + fid + (len|pad) + name
    let words = (total + 3) / 4;

    put_u8(out, OP_OPEN_FONT);
    put_u8(out, 0);
    put_u16(out, words as u16);
    put_u32(out, fid);
    put_u16(out, payload_len as u16);
    put_u16(out, 0); // pad
    out.extend_from_slice(name_bytes);
    align4(out);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} OpenFont fid=0x{:x} name={:?}", seq, fid, name);
    (seq, fid)
}

/// `CreateGlyphCursor`: build a cursor from two glyphs in a font.
/// Phase 19 uses this to replicate openbox's standard pointer
/// (source/mask glyphs 68/69 in the "cursor" font, fg=black, bg=white).
pub fn create_glyph_cursor(out: &mut Vec<u8>, conn: &mut X11Conn,
                           source_font: u32, mask_font: u32,
                           source_char: u16, mask_char: u16,
                           fg_rgb: (u16, u16, u16),
                           bg_rgb: (u16, u16, u16)) -> (u16, u32) {
    let cid = conn.alloc_xid();
    put_u8(out, OP_CREATE_GLYPH_CURSOR);
    put_u8(out, 0);
    put_u16(out, 8);                  // length = 8 words = 32 bytes
    put_u32(out, cid);
    put_u32(out, source_font);
    put_u32(out, mask_font);
    put_u16(out, source_char);
    put_u16(out, mask_char);
    put_u16(out, fg_rgb.0); put_u16(out, fg_rgb.1); put_u16(out, fg_rgb.2);
    put_u16(out, bg_rgb.0); put_u16(out, bg_rgb.1); put_u16(out, bg_rgb.2);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} CreateGlyphCursor cid=0x{:x} src_font=0x{:x} src_char={} mask_char={}",
         seq, cid, source_font, source_char, mask_char);
    (seq, cid)
}

/// `ChangeWindowAttributes` with the cursor attribute.  Used by
/// phase 18 to test whether installing a cursor on root reproduces
/// the openbox-trigger hang.  cursor=0 means "use the default
/// (None) cursor" — Xorg treats it as removing the cursor.
pub fn change_window_attrs_cursor(out: &mut Vec<u8>, conn: &mut X11Conn,
                                  window: u32, cursor: u32) -> u16 {
    put_u8(out, OP_CHANGE_WINDOW_ATTRS);
    put_u8(out, 0);
    put_u16(out, 4);
    put_u32(out, window);
    put_u32(out, CW_CURSOR);
    put_u32(out, cursor);
    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} ChangeWindowAttributes win=0x{:x} CW_CURSOR cursor=0x{:x}",
         seq, window, cursor);
    seq
}

/// `ChangeWindowAttributes` with one attribute set: the event mask.
/// Used by phase 1 to grab `SubstructureRedirectMask | SubstructureNotifyMask`
/// on root, declaring kbox the active WM.
pub fn change_window_attrs_event_mask(out: &mut Vec<u8>, conn: &mut X11Conn,
                                      window: u32, event_mask: u32) -> u16 {
    // Header word + window(4) + value-mask(4) + value(4) = 16 bytes = 4 words.
    put_u8(out, OP_CHANGE_WINDOW_ATTRS);
    put_u8(out, 0);
    put_u16(out, 4);                     // length = 4 words
    put_u32(out, window);
    put_u32(out, CW_EVENT_MASK);
    put_u32(out, event_mask);

    let seq = conn.seq_next;
    conn.seq_next = conn.seq_next.wrapping_add(1);
    req!("seq#{} ChangeWindowAttributes win=0x{:x} event_mask=0x{:x}",
         seq, window, event_mask);
    seq
}
