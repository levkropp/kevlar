// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Request dispatcher.
//
// Phase 1: wire-level scaffolding (log + BadImplementation stub).
// Phase 2: InternAtom (16), GetAtomName (17).
// Phase 3: windows (1-15), properties (18-21), events.
// Phase 4: GCs (55-60), drawing (61-72), colormaps (78-92).
// Phase 5: pixmaps (53, 54), CopyArea across drawables, GetImage (73).
// Phase 6: fonts (45-52) + text ops (74-77) + CreateGlyphCursor (94).
// Phase 7: input (26-38, 41-44, 101-104, 116-119).

use crate::atom;
use crate::client::Client;
use crate::colormap;
use crate::event;
use crate::gc::{self, Gc};
use crate::font::{self, FontRef};
use crate::keymap;
use crate::log;
use crate::pixmap::Pixmap;
use crate::property::{ChangeMode, Property};
use crate::region::{Rect as RegRect, Region};
use crate::render::{self, ClipRect};
use crate::render_ext::{self, Glyph, GlyphSet, Picture};
use crate::resources::{Resource, ResourceMap};
use crate::setup;
use crate::state::ServerState;
use crate::window::{event_mask as em, Window, WindowClass};
use crate::wire::{
    build_error, errcode, get_u16, get_u32, opcode_expects_reply, opcode_name, pad4,
    put_reply_header, put_u16, put_u32, put_u8, RequestHeader,
};

/// Returned by `dispatch_request` to tell the caller how much of the
/// read buffer was consumed.
#[derive(Debug)]
pub enum DispatchResult {
    Consumed(usize),
    NeedMore,
    Fatal(&'static str),
}

pub fn dispatch_request(c: &mut Client, state: &mut ServerState) -> DispatchResult {
    let Some(hdr) = RequestHeader::parse(&c.read_buf) else {
        return DispatchResult::NeedMore;
    };

    // BIG-REQUESTS: when length_words == 0, the real length lives in
    // a 32-bit word at bytes 4..8.  Total request byte count is
    // extended_length * 4, body starts at byte 8.  We normalize this
    // into the shape every handler expects — [opcode, data, 0, 0,
    // body...] — by eliding bytes 4..8 from the copied request.
    let (req_len, body_offset) = if hdr.length_words == 0 {
        if c.read_buf.len() < 8 {
            return DispatchResult::NeedMore;
        }
        let ext = u32::from_le_bytes([
            c.read_buf[4], c.read_buf[5], c.read_buf[6], c.read_buf[7],
        ]) as usize;
        if ext < 2 {
            return DispatchResult::Fatal("BIG-REQUESTS extended length < 2 words");
        }
        (ext * 4, 8)
    } else {
        (hdr.length_bytes(), 4)
    };
    if c.read_buf.len() < req_len {
        return DispatchResult::NeedMore;
    }

    let seq = c.next_seq();
    // Normalize raw: [header 4 bytes] + [body bytes from body_offset..req_len].
    let raw: Vec<u8> = if body_offset == 4 {
        c.read_buf[..req_len].to_vec()
    } else {
        let mut v = Vec::with_capacity(4 + (req_len - body_offset));
        v.extend_from_slice(&c.read_buf[..4]);
        v.extend_from_slice(&c.read_buf[body_offset..req_len]);
        v
    };

    log::req(
        c.id, seq, hdr.opcode, opcode_name(hdr.opcode),
        format_args!("len={} data=0x{:02x}", req_len, hdr.data),
        &raw,
    );

    let handled = match hdr.opcode {
        1  => handle_create_window(c, state, seq, &hdr, &raw),
        2  => handle_change_window_attributes(c, state, seq, &hdr, &raw),
        3  => handle_get_window_attributes(c, state, seq, &raw),
        4  => handle_destroy_window(c, state, seq, &raw),
        5  => handle_destroy_subwindows(c, state, seq, &raw),
        8  => handle_map_window(c, state, seq, &raw),
        9  => handle_map_subwindows(c, state, seq, &raw),
        10 => handle_unmap_window(c, state, seq, &raw),
        12 => handle_configure_window(c, state, seq, &raw),
        14 => handle_get_geometry(c, state, seq, &raw),
        15 => handle_query_tree(c, state, seq, &raw),
        16 => handle_intern_atom(c, state, seq, &hdr, &raw),
        17 => handle_get_atom_name(c, state, seq, &raw),
        18 => handle_change_property(c, state, seq, &hdr, &raw),
        19 => handle_delete_property(c, state, seq, &raw),
        20 => handle_get_property(c, state, seq, &hdr, &raw),
        21 => handle_list_properties(c, state, seq, &raw),
        // Phase 8: window-manager ops
        7  => handle_reparent_window(c, state, seq, &raw),
        13 => handle_circulate_window(c, state, seq, &hdr, &raw),
        22 => handle_set_selection_owner(c, state, seq, &raw),
        23 => handle_get_selection_owner(c, state, seq, &raw),
        24 => handle_convert_selection(c, state, seq, &raw),
        25 => handle_send_event(c, state, seq, &hdr, &raw),
        // Phase 9: extensions + NoOperation
        98  => handle_query_extension(c, state, seq, &raw),
        99  => handle_list_extensions(c, state, seq, &raw),
        127 => handle_no_operation(c, state, seq, &raw),
        // BIG-REQUESTS extension (major opcode 128)
        128 => handle_big_req_enable(c, state, seq, &raw),
        // RENDER extension (major opcode 129)
        129 => handle_render_request(c, state, seq, &hdr, &raw),
        // XFIXES extension (major opcode 130)
        130 => handle_xfixes_request(c, state, seq, &hdr, &raw),
        // Phase 7: input (grabs, pointer, focus, keymap)
        26 => handle_grab_pointer(c, state, seq, &hdr, &raw),
        27 => handle_ungrab_pointer(c, state, seq, &raw),
        28 => handle_grab_button_stub(c, state, seq, &raw),
        29 => handle_ungrab_button_stub(c, state, seq, &raw),
        31 => handle_grab_keyboard(c, state, seq, &hdr, &raw),
        32 => handle_ungrab_keyboard(c, state, seq, &raw),
        33 => handle_grab_key_stub(c, state, seq, &raw),
        34 => handle_ungrab_key_stub(c, state, seq, &raw),
        35 => handle_allow_events_stub(c, state, seq, &raw),
        36 => handle_grab_server_stub(c, state, seq, &raw),
        37 => handle_ungrab_server_stub(c, state, seq, &raw),
        38 => handle_query_pointer(c, state, seq, &raw),
        40 => handle_translate_coordinates(c, state, seq, &raw),
        41 => handle_warp_pointer(c, state, seq, &raw),
        42 => handle_set_input_focus(c, state, seq, &hdr, &raw),
        43 => handle_get_input_focus(c, state, seq, &raw),
        44 => handle_query_keymap(c, state, seq, &raw),
        // Phase 6: fonts
        45 => handle_open_font(c, state, seq, &raw),
        46 => handle_close_font(c, state, seq, &raw),
        47 => handle_query_font(c, state, seq, &raw),
        48 => handle_query_text_extents(c, state, seq, &hdr, &raw),
        49 => handle_list_fonts(c, state, seq, &raw),
        50 => handle_list_fonts_with_info(c, state, seq, &raw),
        51 => handle_set_font_path_stub(c, state, seq, &raw),
        52 => handle_get_font_path_stub(c, state, seq, &raw),
        // Phase 5: pixmaps
        53 => handle_create_pixmap(c, state, seq, &hdr, &raw),
        54 => handle_free_pixmap(c, state, seq, &raw),
        // Phase 4: GCs + drawing + colormaps
        55 => handle_create_gc(c, state, seq, &raw),
        56 => handle_change_gc(c, state, seq, &raw),
        57 => handle_copy_gc(c, state, seq, &raw),
        58 => handle_set_dashes_stub(c, state, seq, &raw),
        59 => handle_set_clip_rectangles(c, state, seq, &hdr, &raw),
        60 => handle_free_gc(c, state, seq, &raw),
        61 => handle_clear_area(c, state, seq, &hdr, &raw),
        62 => handle_copy_area(c, state, seq, &raw),
        64 => handle_poly_point(c, state, seq, &hdr, &raw),
        65 => handle_poly_line(c, state, seq, &hdr, &raw),
        66 => handle_poly_segment(c, state, seq, &raw),
        67 => handle_poly_rectangle(c, state, seq, &raw),
        70 => handle_poly_fill_rectangle(c, state, seq, &raw),
        // Phase 6: text ops
        74 => handle_poly_text_8(c, state, seq, &raw),
        75 => handle_poly_text_16(c, state, seq, &raw),
        76 => handle_image_text_8(c, state, seq, &hdr, &raw),
        77 => handle_image_text_16(c, state, seq, &hdr, &raw),
        72 => handle_put_image(c, state, seq, &hdr, &raw),
        73 => handle_get_image(c, state, seq, &hdr, &raw),
        78 => handle_create_colormap(c, state, seq, &hdr, &raw),
        79 => handle_free_colormap(c, state, seq, &raw),
        84 => handle_alloc_color(c, state, seq, &raw),
        85 => handle_alloc_named_color(c, state, seq, &raw),
        88 => handle_free_colors_stub(c, state, seq, &raw),
        91 => handle_query_colors(c, state, seq, &raw),
        92 => handle_lookup_color(c, state, seq, &raw),
        94 => handle_create_glyph_cursor_stub(c, state, seq, &raw),
        97 => handle_query_best_size(c, state, seq, &raw),
        // Phase 7: keymap / pointer mapping / bell
        101 => handle_get_keyboard_mapping(c, state, seq, &hdr, &raw),
        102 => handle_change_keyboard_control_stub(c, state, seq, &raw),
        103 => handle_get_keyboard_control(c, state, seq, &raw),
        104 => handle_bell_stub(c, state, seq, &raw),
        // Phase 12: miscellaneous query/control batch (for xset/xterm)
        105 => handle_change_pointer_control_stub(c, state, seq, &raw),
        106 => handle_get_pointer_control(c, state, seq, &raw),
        107 => handle_set_screen_saver_stub(c, state, seq, &raw),
        108 => handle_get_screen_saver(c, state, seq, &raw),
        109 => handle_change_hosts_stub(c, state, seq, &raw),
        110 => handle_list_hosts(c, state, seq, &raw),
        111 => handle_set_access_control_stub(c, state, seq, &raw),
        112 => handle_set_close_down_mode_stub(c, state, seq, &raw),
        113 => handle_kill_client_stub(c, state, seq, &raw),
        115 => handle_force_screen_saver_stub(c, state, seq, &raw),
        116 => handle_set_pointer_mapping(c, state, seq, &hdr, &raw),
        117 => handle_get_pointer_mapping(c, state, seq, &raw),
        118 => handle_set_modifier_mapping(c, state, seq, &hdr, &raw),
        119 => handle_get_modifier_mapping(c, state, seq, &raw),
        _  => false,
    };

    if !handled {
        if opcode_expects_reply(hdr.opcode) {
            let err = build_error(errcode::BAD_IMPLEMENTATION, seq, 0, hdr.opcode, 0);
            c.write_buf.extend_from_slice(&err);
            log::err(
                c.id, seq, errcode::BAD_IMPLEMENTATION, 0,
                &format!("opcode {} ({}) not implemented yet",
                         hdr.opcode, opcode_name(hdr.opcode)),
            );
        } else if !c.logged_unhandled[hdr.opcode as usize] {
            c.logged_unhandled[hdr.opcode as usize] = true;
            log::warn(format_args!(
                "silently accepting unhandled opcode {} ({})",
                hdr.opcode, opcode_name(hdr.opcode),
            ));
        }
    }

    c.read_buf.drain(..req_len);
    DispatchResult::Consumed(req_len)
}

// ═════════════════════════════════════════════════════════════════════
// Windows
// ═════════════════════════════════════════════════════════════════════

// CreateWindow (1)
//     byte  0  opcode
//     byte  1  depth
//     word  2  length
//     dword 4  wid (new id for this window)
//     dword 8  parent
//     word 12  x
//     word 14  y
//     word 16  width
//     word 18  height
//     word 20  border-width
//     word 22  class
//     dword24  visual
//     dword28  value-mask
//     bytes 32+  LISTofVALUE (value-mask popcount dwords)
fn handle_create_window(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 32 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 1, 0, "CreateWindow short");
        return true;
    }
    let depth = hdr.data;
    let wid          = get_u32(raw, 4);
    let parent       = get_u32(raw, 8);
    let x            = get_u16(raw, 12) as i16;
    let y            = get_u16(raw, 14) as i16;
    let width        = get_u16(raw, 16);
    let height       = get_u16(raw, 18);
    let border_width = get_u16(raw, 20);
    let class_u16    = get_u16(raw, 22);
    let visual       = get_u32(raw, 24);
    let value_mask   = get_u32(raw, 28);

    // Must belong to this client, must not already exist.
    if !belongs_to_client(c.id, wid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, wid, 1, 0, "CreateWindow id not in client range");
        return true;
    }
    if state.resources.get(wid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, wid, 1, 0, "CreateWindow id already in use");
        return true;
    }
    // Parent must exist.
    if state.resources.window(parent).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, parent, 1, 0, "CreateWindow parent is not a window");
        return true;
    }

    let class = WindowClass::from_u16(class_u16);
    let effective_visual = if visual == 0 { setup::ROOT_VISUAL_ID } else { visual };
    let effective_depth  = if depth == 0 { 24 } else { depth };

    let mut window = Window::new_child(
        wid, parent, c.id,
        x, y, width, height, border_width,
        class, effective_depth, effective_visual,
    );

    // Apply any attributes in the value list.  The value-mask bits
    // mirror ChangeWindowAttributes; we reuse the same parser.
    let values = &raw[32..];
    let result = apply_window_values(&mut window, c.id, value_mask, values);
    let new_mask_at_create = result.new_event_mask;

    // Link into parent's child list.
    if let Some(p) = state.resources.window_mut(parent) {
        p.children.push(wid);
    }

    let override_redirect = window.override_redirect;
    state.resources.insert(wid, c.id, Resource::Window(window));

    if let Some(m) = new_mask_at_create {
        update_redirect_owner(state, wid, c.id, m);
    }

    log::rep(
        c.id, seq,
        format_args!(
            "CreateWindow wid=0x{wid:x} parent=0x{parent:x} {width}x{height}+{x}+{y} depth={effective_depth} class={class_u16}"
        ),
        &[],
    );

    // Generate CreateNotify on the parent for any client that selected
    // SubstructureNotify there.
    let ev = event::create_notify(parent, wid, x, y, width, height, border_width, override_redirect);
    deliver_to_substructure(c, state, parent, em::SUBSTRUCTURE_NOTIFY, ev);
    true
}

// ChangeWindowAttributes (2)
//     dword 4  window
//     dword 8  value-mask
//     bytes 12+ LISTofVALUE
fn handle_change_window_attributes(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 2, 0, "ChangeWindowAttributes short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let value_mask = get_u32(raw, 8);
    let values = &raw[12..];
    let win = match state.resources.window_mut(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 2, 0, "ChangeWindowAttributes bad window");
            return true;
        }
    };
    let result = apply_window_values(win, c.id, value_mask, values);
    if let Some(m) = result.new_event_mask {
        update_redirect_owner(state, wid, c.id, m);
    }
    log::rep(c.id, seq, format_args!("ChangeWindowAttributes wid=0x{wid:x} mask=0x{value_mask:x}"), &[]);
    true
}

/// Result of applying a CWxxx value list.  Carries anything the
/// caller needs to look at after the fact — currently just the new
/// event mask, used by Phase 8 to update `redirect_owners`.
#[derive(Debug, Default, Clone, Copy)]
struct WindowValuesResult {
    pub new_event_mask: Option<u32>,
}

/// Decode a CWxxx value-list and apply it to `win`.  The value-mask
/// bits are the CW* constants from xproto (bit 0 = CWBackPixmap, …).
/// The `caller` id is used when installing a per-client event mask
/// (CWEventMask, bit 11) — multiple clients can select different
/// masks on the same window and each gets its own listener entry.
fn apply_window_values(
    win: &mut Window,
    caller: u32,
    mask: u32,
    mut values: &[u8],
) -> WindowValuesResult {
    let bits = [
        (0,  4),  // CWBackPixmap
        (1,  4),  // CWBackPixel
        (2,  4),  // CWBorderPixmap
        (3,  4),  // CWBorderPixel
        (4,  4),  // CWBitGravity
        (5,  4),  // CWWinGravity
        (6,  4),  // CWBackingStore
        (7,  4),  // CWBackingPlanes
        (8,  4),  // CWBackingPixel
        (9,  4),  // CWOverrideRedirect
        (10, 4),  // CWSaveUnder
        (11, 4),  // CWEventMask
        (12, 4),  // CWDontPropagate
        (13, 4),  // CWColormap
        (14, 4),  // CWCursor
    ];
    let mut result = WindowValuesResult::default();
    for (bit, size) in bits {
        if (mask & (1u32 << bit)) == 0 { continue; }
        if values.len() < size { return result; }
        let v32 = u32::from_le_bytes([values[0], values[1], values[2], values[3]]);
        values = &values[size..];
        match bit {
            1  => win.bg_pixel     = v32,
            3  => win.border_pixel = v32,
            6  => win.backing_store = v32 as u8,
            9  => win.override_redirect = v32 != 0,
            10 => win.save_under    = v32 != 0,
            11 => {
                win.set_listener(caller, v32);
                result.new_event_mask = Some(v32);
            }
            12 => win.do_not_propagate = v32,
            13 => win.colormap = v32,
            14 => win.cursor = v32,
            _  => {}
        }
    }
    result
}

// GetWindowAttributes (3) — reply is 44 bytes total (32 + 12 extra).
fn handle_get_window_attributes(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 3, 0, "GetWindowAttributes short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let win = match state.resources.window(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 3, 0, "GetWindowAttributes bad window");
            return true;
        }
    };
    // Reply length = 3 words of extra beyond the 32-byte minimum.
    let mut out = Vec::with_capacity(44);
    put_reply_header(&mut out, seq, win.backing_store, 3);
    put_u32(&mut out, win.visual);                     // 8
    put_u16(&mut out, class_to_u16(win.class));        // 12
    put_u8(&mut out, 0);                               // 14: bit_gravity
    put_u8(&mut out, 0);                               // 15: win_gravity
    put_u32(&mut out, 0);                              // 16: backing_planes
    put_u32(&mut out, 0);                              // 20: backing_pixel
    put_u8(&mut out, win.save_under as u8);            // 24
    put_u8(&mut out, 1);                               // 25: map_is_installed
    put_u8(&mut out, if win.mapped { 2 } else { 0 });  // 26: map_state (0=Unmapped, 1=Unviewable, 2=Viewable)
    put_u8(&mut out, win.override_redirect as u8);     // 27
    put_u32(&mut out, win.colormap);                   // 28
    put_u32(&mut out, win.combined_mask());            // 32: all_event_masks
    put_u32(&mut out,
        win.listeners.iter().find(|l| l.client == c.id).map(|l| l.mask).unwrap_or(0));  // 36: your_event_mask
    put_u16(&mut out, win.do_not_propagate as u16);    // 40
    put_u16(&mut out, 0);                              // 42: pad
    debug_assert_eq!(out.len(), 44);
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetWindowAttributes wid=0x{wid:x}"), &out);
    true
}

fn class_to_u16(c: WindowClass) -> u16 {
    match c {
        WindowClass::CopyFromParent => 0,
        WindowClass::InputOutput    => 1,
        WindowClass::InputOnly      => 2,
    }
}

// DestroyWindow (4) — recursive destroy of the window and its subtree.
fn handle_destroy_window(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 4, 0, "DestroyWindow short");
        return true;
    }
    let wid = get_u32(raw, 4);
    if wid == setup::ROOT_WINDOW_ID {
        // Silently ignore destroy on root.
        return true;
    }
    if state.resources.window(wid).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, wid, 4, 0, "DestroyWindow bad window");
        return true;
    }
    destroy_window_tree(c, state, wid);
    log::rep(c.id, seq, format_args!("DestroyWindow wid=0x{wid:x}"), &[]);
    true
}

fn destroy_window_tree(c: &mut Client, state: &mut ServerState, wid: u32) {
    // Collect children first (by copy of id list).
    let children: Vec<u32> = state
        .resources
        .window(wid)
        .map(|w| w.children.clone())
        .unwrap_or_default();
    for child in children {
        destroy_window_tree(c, state, child);
    }
    // Unlink from parent.
    let (parent, was_mapped) = {
        let win = state.resources.window(wid).expect("window should exist");
        (win.parent, win.mapped)
    };
    if parent != 0 {
        if let Some(p) = state.resources.window_mut(parent) {
            p.children.retain(|id| *id != wid);
        }
    }
    // Emit UnmapNotify if it was mapped.
    if was_mapped {
        let ev = event::unmap_notify(wid, wid, false);
        deliver_to_structure(c, state, wid, em::STRUCTURE_NOTIFY, ev);
        let ev_parent = event::unmap_notify(parent, wid, false);
        deliver_to_substructure(c, state, parent, em::SUBSTRUCTURE_NOTIFY, ev_parent);
    }
    // Emit DestroyNotify to the window itself and to the parent.
    let ev_self = event::destroy_notify(wid, wid);
    deliver_to_structure(c, state, wid, em::STRUCTURE_NOTIFY, ev_self);
    if parent != 0 {
        let ev_parent = event::destroy_notify(parent, wid);
        deliver_to_substructure(c, state, parent, em::SUBSTRUCTURE_NOTIFY, ev_parent);
    }
    state.resources.remove(wid);
}

fn handle_destroy_subwindows(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 5, 0, "DestroySubwindows short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let children: Vec<u32> = match state.resources.window(wid) {
        Some(w) => w.children.clone(),
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 5, 0, "DestroySubwindows bad window");
            return true;
        }
    };
    for child in children {
        destroy_window_tree(c, state, child);
    }
    log::rep(c.id, seq, format_args!("DestroySubwindows wid=0x{wid:x}"), &[]);
    true
}

// MapWindow (8)
fn handle_map_window(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 8, 0, "MapWindow short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let (parent, override_redirect, was_mapped, x, y, w_, h_) = {
        let win = match state.resources.window(wid) {
            Some(w) => w,
            None => {
                send_error(c, errcode::BAD_WINDOW, seq, wid, 8, 0, "MapWindow bad window");
                return true;
            }
        };
        (win.parent, win.override_redirect, win.mapped, win.x, win.y, win.width, win.height)
    };
    // ── SubstructureRedirect interception ─────────────────────
    // If the parent has a redirect owner other than us, and this
    // window does NOT have override_redirect set, synthesize
    // MapRequest to the owner and DO NOT apply the state change.
    if !override_redirect {
        if let Some(owner) = find_redirect_owner(state, parent) {
            if owner != c.id {
                let ev = event::map_request(parent, wid);
                queue_event_to_client(c, state, owner, ev);
                log::rep(c.id, seq, format_args!(
                    "MapWindow wid={wid:#x} → MapRequest to C{owner}"
                ), &[]);
                return true;
            }
        }
    }
    if was_mapped {
        log::rep(c.id, seq, format_args!("MapWindow wid=0x{wid:x} (already mapped)"), &[]);
        return true;
    }
    if let Some(win) = state.resources.window_mut(wid) {
        win.mapped = true;
    }
    // Paint the window's bg_pixel BEFORE generating Expose.  The X11
    // spec allows the server to leave contents undefined on first
    // map, but the common real-server behavior (and what clients
    // like xlib's XDrawString-over-default-GC implicitly depend on)
    // is to paint the full window with its background pixel first,
    // THEN emit Expose so the client can draw on top.
    let bg_pixel = state.resources.window(wid).map(|w| w.bg_pixel).unwrap_or(0);
    let abs = abs_origin(state, wid);
    if let Some(clip) = window_clip(state, wid) {
        render::fill_rect_window(
            &mut state.fb, abs, clip, 0, 0, w_, h_, bg_pixel,
        );
    }
    // MapNotify → the window itself + parent's substructure listeners.
    let ev = event::map_notify(wid, wid, override_redirect);
    deliver_to_structure(c, state, wid, em::STRUCTURE_NOTIFY, ev);
    if parent != 0 {
        let ev_parent = event::map_notify(parent, wid, override_redirect);
        deliver_to_substructure(c, state, parent, em::SUBSTRUCTURE_NOTIFY, ev_parent);
    }
    // Expose: a single rectangle covering the whole window.
    let expose = event::expose(wid, 0, 0, w_, h_, 0);
    deliver_to_structure(c, state, wid, em::EXPOSURE, expose);
    // Suppress unused warning on x/y:
    let _ = (x, y);
    log::rep(c.id, seq, format_args!("MapWindow wid=0x{wid:x}"), &[]);
    true
}

fn handle_map_subwindows(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 9, 0, "MapSubwindows short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let children: Vec<u32> = match state.resources.window(wid) {
        Some(w) => w.children.clone(),
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 9, 0, "MapSubwindows bad window");
            return true;
        }
    };
    // Cheap: just call handle_map_window for each (with synthetic raw).
    for child in children {
        let mut synth = [0u8; 8];
        synth[0] = 8;
        synth[2] = 2; // length 2 words
        synth[4..8].copy_from_slice(&child.to_le_bytes());
        handle_map_window(c, state, seq, &synth);
    }
    log::rep(c.id, seq, format_args!("MapSubwindows wid=0x{wid:x}"), &[]);
    true
}

// UnmapWindow (10)
fn handle_unmap_window(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 10, 0, "UnmapWindow short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let (parent, was_mapped) = {
        let win = match state.resources.window(wid) {
            Some(w) => w,
            None => {
                send_error(c, errcode::BAD_WINDOW, seq, wid, 10, 0, "UnmapWindow bad window");
                return true;
            }
        };
        (win.parent, win.mapped)
    };
    if !was_mapped {
        return true;
    }
    if let Some(win) = state.resources.window_mut(wid) {
        win.mapped = false;
    }
    let ev = event::unmap_notify(wid, wid, false);
    deliver_to_structure(c, state, wid, em::STRUCTURE_NOTIFY, ev);
    if parent != 0 {
        let ev_parent = event::unmap_notify(parent, wid, false);
        deliver_to_substructure(c, state, parent, em::SUBSTRUCTURE_NOTIFY, ev_parent);
    }
    log::rep(c.id, seq, format_args!("UnmapWindow wid=0x{wid:x}"), &[]);
    true
}

// ConfigureWindow (12)
fn handle_configure_window(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 12, 0, "ConfigureWindow short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let mask = get_u16(raw, 8);
    // ── SubstructureRedirect interception ─────────────────────
    // If someone else holds SubstructureRedirect on our parent and
    // we're not override_redirect, synthesize a ConfigureRequest
    // instead of applying.  The ConfigureRequest carries the same
    // value mask + values the client sent, so the WM can decide.
    let (parent, override_redirect) = match state.resources.window(wid) {
        Some(w) => (w.parent, w.override_redirect),
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 12, 0, "ConfigureWindow bad window");
            return true;
        }
    };
    if !override_redirect {
        if let Some(owner) = find_redirect_owner(state, parent) {
            if owner != c.id {
                // Parse the value list into individual fields for the
                // ConfigureRequest event body.
                let mut values = &raw[12..];
                let take = |values: &mut &[u8]| -> u32 {
                    if values.len() < 4 { return 0; }
                    let v = u32::from_le_bytes([values[0], values[1], values[2], values[3]]);
                    *values = &values[4..];
                    v
                };
                let mut x = 0i16; let mut y = 0i16;
                let mut w_ = 0u16; let mut h_ = 0u16;
                let mut bw = 0u16; let mut sibling = 0u32;
                let mut stack_mode = 0u8;
                if mask & 0x01 != 0 { x = take(&mut values) as i16; }
                if mask & 0x02 != 0 { y = take(&mut values) as i16; }
                if mask & 0x04 != 0 { w_ = take(&mut values) as u16; }
                if mask & 0x08 != 0 { h_ = take(&mut values) as u16; }
                if mask & 0x10 != 0 { bw = take(&mut values) as u16; }
                if mask & 0x20 != 0 { sibling = take(&mut values); }
                if mask & 0x40 != 0 { stack_mode = take(&mut values) as u8; }
                let ev = event::configure_request(
                    stack_mode, parent, wid, sibling, x, y, w_, h_, bw, mask,
                );
                queue_event_to_client(c, state, owner, ev);
                log::rep(c.id, seq, format_args!(
                    "ConfigureWindow wid={wid:#x} → ConfigureRequest to C{owner}"
                ), &[]);
                return true;
            }
        }
    }
    let mut values = &raw[12..];
    let win = match state.resources.window_mut(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 12, 0, "ConfigureWindow bad window");
            return true;
        }
    };
    // Bits: 0=x 1=y 2=width 3=height 4=border_width 5=sibling 6=stack-mode
    let take = |values: &mut &[u8]| -> u32 {
        if values.len() < 4 { return 0; }
        let v = u32::from_le_bytes([values[0], values[1], values[2], values[3]]);
        *values = &values[4..];
        v
    };
    if mask & 0x01 != 0 { win.x = take(&mut values) as i16; }
    if mask & 0x02 != 0 { win.y = take(&mut values) as i16; }
    if mask & 0x04 != 0 { win.width = take(&mut values) as u16; }
    if mask & 0x08 != 0 { win.height = take(&mut values) as u16; }
    if mask & 0x10 != 0 { win.border_width = take(&mut values) as u16; }
    // sibling/stack-mode: ignore for Phase 3
    if mask & 0x20 != 0 { let _ = take(&mut values); }
    if mask & 0x40 != 0 { let _ = take(&mut values); }
    let (parent, override_redirect, x, y, w_, h_, bw) = {
        let win = state.resources.window(wid).unwrap();
        (win.parent, win.override_redirect, win.x, win.y, win.width, win.height, win.border_width)
    };
    let ev = event::configure_notify(wid, wid, 0, x, y, w_, h_, bw, override_redirect);
    deliver_to_structure(c, state, wid, em::STRUCTURE_NOTIFY, ev);
    if parent != 0 {
        let ev_parent = event::configure_notify(parent, wid, 0, x, y, w_, h_, bw, override_redirect);
        deliver_to_substructure(c, state, parent, em::SUBSTRUCTURE_NOTIFY, ev_parent);
    }
    log::rep(c.id, seq, format_args!("ConfigureWindow wid=0x{wid:x} mask=0x{mask:x}"), &[]);
    true
}

// GetGeometry (14) — reply is 32 bytes total (0 extra words).
//     depth            byte 1 (in reply header)
//     root             4 bytes (8..12)
//     x                2 bytes (12..14)
//     y                2 bytes (14..16)
//     width            2 bytes (16..18)
//     height           2 bytes (18..20)
//     border_width     2 bytes (20..22)
//     10 bytes pad
fn handle_get_geometry(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 14, 0, "GetGeometry short");
        return true;
    }
    let did = get_u32(raw, 4);
    let win = match state.resources.window(did) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_DRAWABLE, seq, did, 14, 0, "GetGeometry bad drawable");
            return true;
        }
    };
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, win.depth, 0);
    put_u32(&mut out, setup::ROOT_WINDOW_ID);
    put_u16(&mut out, win.x as u16);
    put_u16(&mut out, win.y as u16);
    put_u16(&mut out, win.width);
    put_u16(&mut out, win.height);
    put_u16(&mut out, win.border_width);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(
        c.id, seq,
        format_args!("GetGeometry 0x{did:x} → {}x{}+{}+{} depth={}",
                     win.width, win.height, win.x, win.y, win.depth),
        &out,
    );
    true
}

// QueryTree (15)
//     reply header (8)
//     root            4 bytes
//     parent          4 bytes (0 if root)
//     num_children    2 bytes
//     14 bytes pad
//     LISTofWINDOW    4*n bytes
fn handle_query_tree(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 15, 0, "QueryTree short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let win = match state.resources.window(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 15, 0, "QueryTree bad window");
            return true;
        }
    };
    let children = win.children.clone();
    let parent = win.parent;
    let n = children.len();
    let extra_words = n as u32; // each child is a dword
    let mut out = Vec::with_capacity(32 + 4 * n);
    put_reply_header(&mut out, seq, 0, extra_words);
    put_u32(&mut out, setup::ROOT_WINDOW_ID);
    put_u32(&mut out, parent);
    put_u16(&mut out, n as u16);
    for _ in 0..14 { out.push(0); }
    debug_assert_eq!(out.len(), 32);
    for ch in &children {
        put_u32(&mut out, *ch);
    }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("QueryTree 0x{wid:x} → {} children", n), &out);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Atoms (Phase 2, unchanged)
// ═════════════════════════════════════════════════════════════════════

fn handle_intern_atom(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 16, 0, "InternAtom short");
        return true;
    }
    let only_if_exists = hdr.data != 0;
    let name_len = get_u16(raw, 4) as usize;
    if raw.len() < 8 + name_len {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 16, 0, "InternAtom name overflow");
        return true;
    }
    let name_bytes = &raw[8..8 + name_len];
    let name = match core::str::from_utf8(name_bytes) {
        Ok(s) => s,
        Err(_) => {
            send_error(c, errcode::BAD_VALUE, seq, 0, 16, 0, "InternAtom non-utf8");
            return true;
        }
    };
    let atom_id = state.atoms.intern(name, only_if_exists).unwrap_or(0);
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u32(&mut out, atom_id);
    out.resize(32, 0);
    c.write_buf.extend_from_slice(&out);
    log::rep(
        c.id, seq,
        format_args!("InternAtom name={name:?} only_if_exists={only_if_exists} → atom={atom_id}"),
        &out,
    );
    true
}

fn handle_get_atom_name(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 17, 0, "GetAtomName short");
        return true;
    }
    let atom_id = get_u32(raw, 4);
    let name = match state.atoms.name(atom_id) {
        Some(s) => s.to_string(),
        None => {
            send_error(c, errcode::BAD_ATOM, seq, atom_id, 17, 0, "GetAtomName unknown atom");
            return true;
        }
    };
    let n = name.len();
    let padded = pad4(n);
    let extra_words = (padded / 4) as u32;
    let mut out = Vec::with_capacity(32 + padded);
    put_reply_header(&mut out, seq, 0, extra_words);
    put_u16(&mut out, n as u16);
    for _ in 0..22 { out.push(0); }
    debug_assert_eq!(out.len(), 32);
    out.extend_from_slice(name.as_bytes());
    while out.len() % 4 != 0 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetAtomName atom={atom_id} → {name:?}"), &out);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Properties
// ═════════════════════════════════════════════════════════════════════

// ChangeProperty (18)
//     byte  0  opcode
//     byte  1  mode (0=Replace, 1=Prepend, 2=Append)
//     word  2  length
//     dword 4  window
//     dword 8  property atom
//     dword 12 type atom
//     byte  16 format (8/16/32)
//     3 bytes pad
//     dword 20 length-of-data in format units
//     bytes 24+ data, padded to 4
fn handle_change_property(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 24 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 18, 0, "ChangeProperty short");
        return true;
    }
    let mode = match ChangeMode::from_u8(hdr.data) {
        Some(m) => m,
        None => {
            send_error(c, errcode::BAD_VALUE, seq, 0, 18, 0, "ChangeProperty bad mode");
            return true;
        }
    };
    let wid       = get_u32(raw, 4);
    let prop_atom = get_u32(raw, 8);
    let type_atom = get_u32(raw, 12);
    let format    = raw[16];
    let n_items   = get_u32(raw, 20) as usize;
    if format != 8 && format != 16 && format != 32 {
        send_error(c, errcode::BAD_VALUE, seq, 0, 18, 0, "ChangeProperty bad format");
        return true;
    }
    let byte_len = n_items * (format as usize / 8);
    if raw.len() < 24 + byte_len {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 18, 0, "ChangeProperty data overflow");
        return true;
    }
    let data = raw[24..24 + byte_len].to_vec();

    let win = match state.resources.window_mut(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 18, 0, "ChangeProperty bad window");
            return true;
        }
    };

    // Find existing property by name atom.
    let existing = win.properties.iter().position(|p| p.name == prop_atom);
    match (mode, existing) {
        (ChangeMode::Replace, Some(pos)) => {
            win.properties[pos] = Property { name: prop_atom, ty: type_atom, format, data };
        }
        (ChangeMode::Replace, None) => {
            win.properties.push(Property { name: prop_atom, ty: type_atom, format, data });
        }
        (ChangeMode::Prepend, Some(pos)) => {
            let mut combined = data;
            combined.extend_from_slice(&win.properties[pos].data);
            win.properties[pos].data = combined;
        }
        (ChangeMode::Append, Some(pos)) => {
            win.properties[pos].data.extend_from_slice(&data);
        }
        (_, None) => {
            win.properties.push(Property { name: prop_atom, ty: type_atom, format, data });
        }
    }

    let ev = event::property_notify(wid, prop_atom, false);
    deliver_to_structure(c, state, wid, em::PROPERTY_CHANGE, ev);
    log::rep(
        c.id, seq,
        format_args!("ChangeProperty wid=0x{wid:x} prop={prop_atom} type={type_atom} fmt={format} n={n_items} mode={mode:?}"),
        &[],
    );
    true
}

// DeleteProperty (19)
fn handle_delete_property(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 19, 0, "DeleteProperty short");
        return true;
    }
    let wid  = get_u32(raw, 4);
    let atom = get_u32(raw, 8);
    let removed = {
        let win = match state.resources.window_mut(wid) {
            Some(w) => w,
            None => {
                send_error(c, errcode::BAD_WINDOW, seq, wid, 19, 0, "DeleteProperty bad window");
                return true;
            }
        };
        let before = win.properties.len();
        win.properties.retain(|p| p.name != atom);
        before != win.properties.len()
    };
    if removed {
        let ev = event::property_notify(wid, atom, true);
        deliver_to_structure(c, state, wid, em::PROPERTY_CHANGE, ev);
    }
    log::rep(c.id, seq, format_args!("DeleteProperty wid=0x{wid:x} atom={atom} removed={removed}"), &[]);
    true
}

// GetProperty (20)
//     byte  0  opcode
//     byte  1  delete flag
//     word  2  length = 6
//     dword 4  window
//     dword 8  property
//     dword 12 type (or 0 for AnyProperty)
//     dword 16 long_offset
//     dword 20 long_length
//
// Reply (32 + padded data):
//     byte  1 → format
//     word  4 → extra words
//     dword 8 → type atom
//     dword 12 → bytes_after (0 when we return everything we have)
//     dword 16 → length returned (in format units)
//     12 bytes pad
//     data + pad
fn handle_get_property(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 24 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 20, 0, "GetProperty short");
        return true;
    }
    let delete = hdr.data != 0;
    let wid         = get_u32(raw, 4);
    let prop_atom   = get_u32(raw, 8);
    let type_filter = get_u32(raw, 12);
    let long_offset = get_u32(raw, 16) as usize;
    let long_length = get_u32(raw, 20) as usize;

    let win = match state.resources.window_mut(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 20, 0, "GetProperty bad window");
            return true;
        }
    };
    let pos = win.properties.iter().position(|p| p.name == prop_atom);
    match pos {
        None => {
            // Reply with format=0, type=0, nothing.
            let mut out = Vec::with_capacity(32);
            put_reply_header(&mut out, seq, 0, 0);
            put_u32(&mut out, 0);   // type = None
            put_u32(&mut out, 0);   // bytes_after
            put_u32(&mut out, 0);   // length
            while out.len() < 32 { out.push(0); }
            c.write_buf.extend_from_slice(&out);
            log::rep(c.id, seq, format_args!("GetProperty 0x{wid:x} atom={prop_atom} (not found)"), &out);
            return true;
        }
        Some(idx) => {
            let p = &win.properties[idx];
            if type_filter != 0 && type_filter != p.ty {
                // Type mismatch — per spec, return the existing type,
                // format=p.format, length=0, bytes_after=total bytes.
                let mut out = Vec::with_capacity(32);
                put_reply_header(&mut out, seq, p.format, 0);
                put_u32(&mut out, p.ty);
                put_u32(&mut out, p.data.len() as u32);  // bytes_after
                put_u32(&mut out, 0);                    // length
                while out.len() < 32 { out.push(0); }
                c.write_buf.extend_from_slice(&out);
                log::rep(c.id, seq, format_args!("GetProperty 0x{wid:x} type mismatch"), &out);
                return true;
            }

            let unit = (p.format / 8) as usize;
            let start = (long_offset * 4).min(p.data.len());
            let wanted_bytes = long_length * 4;
            let end = (start + wanted_bytes).min(p.data.len());
            let slice = &p.data[start..end];
            // Truncate to whole units.
            let unit_end = slice.len() - (slice.len() % unit.max(1));
            let slice = &slice[..unit_end];
            let bytes_after = (p.data.len() - end) as u32;
            let length_units = (slice.len() / unit.max(1)) as u32;
            let padded = pad4(slice.len());
            let extra_words = (padded / 4) as u32;

            let mut out = Vec::with_capacity(32 + padded);
            put_reply_header(&mut out, seq, p.format, extra_words);
            put_u32(&mut out, p.ty);
            put_u32(&mut out, bytes_after);
            put_u32(&mut out, length_units);
            for _ in 0..12 { out.push(0); }
            debug_assert_eq!(out.len(), 32);
            out.extend_from_slice(slice);
            while out.len() % 4 != 0 { out.push(0); }
            c.write_buf.extend_from_slice(&out);

            let log_summary = format!(
                "GetProperty 0x{wid:x} atom={prop_atom} type={} fmt={} len={} after={}",
                p.ty, p.format, length_units, bytes_after
            );
            log::rep(c.id, seq, format_args!("{log_summary}"), &out);

            // DELETE flag: if the full property was returned, drop it.
            if delete && bytes_after == 0 && type_filter == 0 {
                win.properties.remove(idx);
                let ev = event::property_notify(wid, prop_atom, true);
                deliver_to_structure(c, state, wid, em::PROPERTY_CHANGE, ev);
            }
        }
    }
    true
}

// ListProperties (21)
fn handle_list_properties(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 21, 0, "ListProperties short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let win = match state.resources.window(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 21, 0, "ListProperties bad window");
            return true;
        }
    };
    let n = win.properties.len();
    let atoms: Vec<u32> = win.properties.iter().map(|p| p.name).collect();
    let extra_words = n as u32;
    let mut out = Vec::with_capacity(32 + 4 * n);
    put_reply_header(&mut out, seq, 0, extra_words);
    put_u16(&mut out, n as u16);
    for _ in 0..22 { out.push(0); }
    debug_assert_eq!(out.len(), 32);
    for a in &atoms {
        put_u32(&mut out, *a);
    }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("ListProperties 0x{wid:x} → {n} atoms"), &out);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Event delivery helpers
// ═════════════════════════════════════════════════════════════════════

/// Deliver an event to every listener on `target_window` whose mask
/// matches `required_mask_bit`.
fn deliver_to_structure(c: &mut Client, state: &mut ServerState, target_window: u32, required: u32, ev: [u8; 32]) {
    let listeners: Vec<(u32, u32)> = state
        .resources
        .window(target_window)
        .map(|w| w.listeners.iter().map(|l| (l.client, l.mask)).collect())
        .unwrap_or_default();
    for (client_id, mask) in listeners {
        if mask & required != 0 {
            queue_event_to_client(c, state, client_id, ev);
        }
    }
}

/// Deliver an event to every listener on `parent_window` whose mask
/// matches `required_mask_bit` (used for SubstructureNotify).
fn deliver_to_substructure(c: &mut Client, state: &mut ServerState, parent_window: u32, required: u32, ev: [u8; 32]) {
    deliver_to_structure(c, state, parent_window, required, ev);
}

/// Push an event to a client by id.  If the id matches the currently
/// dispatching client, push directly onto its queue.  Otherwise stage
/// it in `state.pending_events` and let `server::poll_once` route it
/// to the right client after the current dispatch unwinds.
fn queue_event_to_client(
    c: &mut Client,
    state: &mut ServerState,
    target_client: u32,
    ev: [u8; 32],
) {
    if target_client == c.id {
        c.queue_event(ev);
    } else {
        state.pending_events.push(crate::state::PendingEvent {
            target_client,
            ev,
        });
    }
}

// ═════════════════════════════════════════════════════════════════════
// Helpers
// ═════════════════════════════════════════════════════════════════════

fn send_error(
    c: &mut Client,
    code: u8,
    seq: u16,
    bad_resource: u32,
    major: u8,
    minor: u16,
    reason: &str,
) {
    let err = build_error(code, seq, bad_resource, major, minor);
    c.write_buf.extend_from_slice(&err);
    log::err(c.id, seq, code, bad_resource, reason);
}

/// Verify that an XID is in the given client's allocation range.
fn belongs_to_client(client_id: u32, xid: u32) -> bool {
    (xid >> 21) == client_id
}

/// Compute a window's absolute screen origin by walking the parent
/// chain from `wid` to the root.  Returns `(abs_x, abs_y)`.
fn abs_origin(state: &ServerState, wid: u32) -> (i32, i32) {
    let mut x = 0i32;
    let mut y = 0i32;
    let mut cur = wid;
    while let Some(w) = state.resources.window(cur) {
        x += w.x as i32;
        y += w.y as i32;
        if w.parent == 0 { break; }
        cur = w.parent;
    }
    (x, y)
}

/// Build a clip rect covering the entire area of a window in screen
/// coordinates.  Used as the default clip for all drawing ops.
fn window_clip(state: &ServerState, wid: u32) -> Option<ClipRect> {
    let w = state.resources.window(wid)?;
    let (ax, ay) = abs_origin(state, wid);
    Some(ClipRect::new(ax, ay, w.width as i32, w.height as i32))
}

// ═════════════════════════════════════════════════════════════════════
// Graphics Contexts (Phase 4)
// ═════════════════════════════════════════════════════════════════════

// CreateGC (55)
//     dword 4  cid (new GC id)
//     dword 8  drawable
//     dword 12 value-mask
//     bytes 16+ value list
fn handle_create_gc(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 55, 0, "CreateGC short");
        return true;
    }
    let cid = get_u32(raw, 4);
    let drawable = get_u32(raw, 8);
    let mask = get_u32(raw, 12);
    let values = &raw[16..];
    if !belongs_to_client(c.id, cid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, cid, 55, 0, "CreateGC id not in client range");
        return true;
    }
    if state.resources.get(cid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, cid, 55, 0, "CreateGC id in use");
        return true;
    }
    if !is_drawable(state, drawable) {
        send_error(c, errcode::BAD_DRAWABLE, seq, drawable, 55, 0, "CreateGC bad drawable");
        return true;
    }
    let mut g = Gc::new_default(drawable);
    gc::apply_values(&mut g, mask, values);
    state.resources.insert(cid, c.id, Resource::Gc(g));
    log::rep(c.id, seq, format_args!("CreateGC gid=0x{cid:x} drawable=0x{drawable:x} mask=0x{mask:x}"), &[]);
    true
}

// ChangeGC (56)
fn handle_change_gc(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 56, 0, "ChangeGC short");
        return true;
    }
    let cid = get_u32(raw, 4);
    let mask = get_u32(raw, 8);
    let values = &raw[12..];
    let g = match state.resources.gc_mut(cid) {
        Some(g) => g,
        None => {
            send_error(c, errcode::BAD_GCONTEXT, seq, cid, 56, 0, "ChangeGC bad gc");
            return true;
        }
    };
    gc::apply_values(g, mask, values);
    log::rep(c.id, seq, format_args!("ChangeGC gid=0x{cid:x} mask=0x{mask:x}"), &[]);
    true
}

// CopyGC (57)
fn handle_copy_gc(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 57, 0, "CopyGC short");
        return true;
    }
    let src_id = get_u32(raw, 4);
    let dst_id = get_u32(raw, 8);
    let _mask  = get_u32(raw, 12);
    let src = match state.resources.gc(src_id) {
        Some(g) => g.clone(),
        None => {
            send_error(c, errcode::BAD_GCONTEXT, seq, src_id, 57, 0, "CopyGC bad src");
            return true;
        }
    };
    match state.resources.gc_mut(dst_id) {
        Some(g) => { *g = src; }
        None => {
            send_error(c, errcode::BAD_GCONTEXT, seq, dst_id, 57, 0, "CopyGC bad dst");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!("CopyGC {src_id:#x} → {dst_id:#x}"), &[]);
    true
}

// SetDashes (58) — stub
fn handle_set_dashes_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("SetDashes (stub)"), &[]);
    true
}

// SetClipRectangles (59)
fn handle_set_clip_rectangles(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 59, 0, "SetClipRectangles short");
        return true;
    }
    let _ordering = hdr.data;
    let gid = get_u32(raw, 4);
    let clip_x = get_u16(raw, 8) as i16;
    let clip_y = get_u16(raw, 10) as i16;
    let rect_bytes = &raw[12..];
    let n_rects = rect_bytes.len() / 8;
    let mut rects = Vec::with_capacity(n_rects);
    for i in 0..n_rects {
        let off = i * 8;
        let x = i16::from_le_bytes([rect_bytes[off], rect_bytes[off+1]]);
        let y = i16::from_le_bytes([rect_bytes[off+2], rect_bytes[off+3]]);
        let w = u16::from_le_bytes([rect_bytes[off+4], rect_bytes[off+5]]);
        let h = u16::from_le_bytes([rect_bytes[off+6], rect_bytes[off+7]]);
        rects.push((x, y, w, h));
    }
    let g = match state.resources.gc_mut(gid) {
        Some(g) => g,
        None => {
            send_error(c, errcode::BAD_GCONTEXT, seq, gid, 59, 0, "SetClipRectangles bad gc");
            return true;
        }
    };
    g.clip_x_origin = clip_x;
    g.clip_y_origin = clip_y;
    g.clip_rects = rects;
    log::rep(c.id, seq, format_args!("SetClipRectangles gid={gid:#x} n={n_rects}"), &[]);
    true
}

// FreeGC (60)
fn handle_free_gc(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 60, 0, "FreeGC short");
        return true;
    }
    let gid = get_u32(raw, 4);
    state.resources.remove(gid);
    log::rep(c.id, seq, format_args!("FreeGC {gid:#x}"), &[]);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Drawing
// ═════════════════════════════════════════════════════════════════════

// ClearArea (61)
//     byte 1: exposures flag
//     dword 4: window
//     word 8: x   word 10: y   word 12: width   word 14: height
fn handle_clear_area(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 61, 0, "ClearArea short");
        return true;
    }
    let exposures = hdr.data != 0;
    let wid = get_u32(raw, 4);
    let x = get_u16(raw, 8)  as i16;
    let y = get_u16(raw, 10) as i16;
    let mut w = get_u16(raw, 12);
    let mut h = get_u16(raw, 14);
    let win = match state.resources.window(wid) {
        Some(w) => w,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 61, 0, "ClearArea bad window");
            return true;
        }
    };
    if w == 0 { w = win.width.saturating_sub(x.max(0) as u16); }
    if h == 0 { h = win.height.saturating_sub(y.max(0) as u16); }
    let bg = win.bg_pixel;
    let mapped = win.mapped;
    let clip = window_clip(state, wid).unwrap();
    let abs = abs_origin(state, wid);
    if mapped {
        render::fill_rect_window(&mut state.fb, abs, clip, x, y, w, h, bg);
    }
    if exposures {
        let ev = event::expose(wid, x.max(0) as u16, y.max(0) as u16, w, h, 0);
        deliver_to_structure(c, state, wid, em::EXPOSURE, ev);
    }
    log::rep(c.id, seq, format_args!("ClearArea wid={wid:#x} {w}x{h}+{x}+{y} bg={bg:#x}"), &[]);
    true
}

// CopyArea (62) — supports all four (window, pixmap) × (window, pixmap)
// combinations via an intermediate Vec<u32> staging buffer.  This
// deliberately sacrifices speed for borrow-checker simplicity: the src
// and dst may alias the same resource, so we read first, then write.
fn handle_copy_area(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 28 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 62, 0, "CopyArea short");
        return true;
    }
    let src_id = get_u32(raw, 4);
    let dst_id = get_u32(raw, 8);
    let _gc    = get_u32(raw, 12);
    let sx = get_u16(raw, 16) as i16;
    let sy = get_u16(raw, 18) as i16;
    let dx = get_u16(raw, 20) as i16;
    let dy = get_u16(raw, 22) as i16;
    let w  = get_u16(raw, 24);
    let h  = get_u16(raw, 26);

    if !is_drawable(state, src_id) {
        send_error(c, errcode::BAD_DRAWABLE, seq, src_id, 62, 0, "CopyArea bad src");
        return true;
    }
    if !is_drawable(state, dst_id) {
        send_error(c, errcode::BAD_DRAWABLE, seq, dst_id, 62, 0, "CopyArea bad dst");
        return true;
    }

    // Stage source pixels into a row-major Vec.  Out-of-bounds source
    // pixels read as 0 (black) — X11 technically specifies GraphicsExpose
    // for obscured src regions, which we punt on.
    let staged = read_rect(state, src_id, sx as i32, sy as i32, w, h);

    // Blit staged buffer into destination, clipped to its bounds.
    write_rect(state, dst_id, dx as i32, dy as i32, w, h, &staged);

    log::rep(c.id, seq, format_args!("CopyArea {src_id:#x}→{dst_id:#x} {w}x{h}"), &[]);
    true
}

/// Read a `w×h` rectangle from any drawable (window or pixmap) into a
/// row-major `Vec<u32>` of length `w*h`.  Source coordinates are
/// drawable-local; out-of-bounds reads yield 0.
fn read_rect(
    state: &ServerState,
    did: u32,
    x: i32, y: i32,
    w: u16, h: u16,
) -> Vec<u32> {
    let mut out = vec![0u32; w as usize * h as usize];
    if let Some(p) = state.resources.pixmap(did) {
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                out[(row * w as i32 + col) as usize] = p.get(x + col, y + row);
            }
        }
        return out;
    }
    if state.resources.window(did).is_some() {
        let abs = abs_origin(state, did);
        let clip = window_clip(state, did).unwrap();
        let fb_w = state.fb.width as i32;
        let fb_h = state.fb.height as i32;
        let stride = state.fb.stride_px;
        // SAFETY: we borrow state.fb immutably via a transmute through
        // a pointer because pixels_mut() takes &mut; but read_rect is
        // called before any write_rect that touches the same surface.
        // Easier: use an inherent read helper.
        let pix = fb_pixels_read(state);
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                let sx = abs.0 + x + col;
                let sy = abs.1 + y + row;
                let in_clip = clip.contains(sx, sy) && sx >= 0 && sy >= 0 && sx < fb_w && sy < fb_h;
                if in_clip {
                    out[(row * w as i32 + col) as usize] = pix[sy as usize * stride + sx as usize];
                }
            }
        }
    }
    out
}

/// Immutable view of the framebuffer's pixel slice.  Used by
/// `read_rect` for window sources — we would otherwise need
/// `pixels_mut()` which requires `&mut state.fb`, conflicting with the
/// immutable `state` borrow we already hold.
fn fb_pixels_read(state: &ServerState) -> &[u32] {
    // We cast through the public API by re-using the stride field.
    // The Framebuffer already exposes a read path through `pixels_mut()`
    // but that wants &mut self.  Add an inherent `pixels_read()` there.
    state.fb.pixels_read()
}

/// Write a `w×h` rectangle of pixels into any drawable at (x, y).
/// For windows the pixels are placed on the framebuffer through the
/// usual abs_origin + clip pipeline; for pixmaps they land in the
/// owned pixel buffer directly.
fn write_rect(
    state: &mut ServerState,
    did: u32,
    x: i32, y: i32,
    w: u16, h: u16,
    src: &[u32],
) {
    if state.resources.pixmap(did).is_some() {
        let p = state.resources.pixmap_mut(did).unwrap();
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                p.put(x + col, y + row, src[(row * w as i32 + col) as usize]);
            }
        }
        return;
    }
    if state.resources.window(did).is_some() {
        let abs = abs_origin(state, did);
        let clip = window_clip(state, did).unwrap();
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                let p = src[(row * w as i32 + col) as usize];
                render::put_pixel_clipped(
                    &mut state.fb,
                    clip,
                    abs.0 + x + col,
                    abs.1 + y + row,
                    p,
                );
            }
        }
    }
}

// PolyPoint (64)
fn handle_poly_point(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 64, 0, "PolyPoint short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let (fg, clip, abs) = match fetch_draw_context(c, state, seq, wid, gid, 64) {
        Some(v) => v,
        None => return true,
    };
    let points = &raw[12..];
    let n = points.len() / 4;
    for i in 0..n {
        let x = i16::from_le_bytes([points[i*4], points[i*4+1]]);
        let y = i16::from_le_bytes([points[i*4+2], points[i*4+3]]);
        render::put_pixel_clipped(&mut state.fb, clip, abs.0 + x as i32, abs.1 + y as i32, fg);
    }
    log::rep(c.id, seq, format_args!("PolyPoint wid={wid:#x} n={n}"), &[]);
    true
}

// PolyLine (65)
fn handle_poly_line(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 65, 0, "PolyLine short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let (fg, clip, abs) = match fetch_draw_context(c, state, seq, wid, gid, 65) {
        Some(v) => v,
        None => return true,
    };
    let pts = &raw[12..];
    let n = pts.len() / 4;
    if n < 2 { return true; }
    let mut prev_x = i16::from_le_bytes([pts[0], pts[1]]);
    let mut prev_y = i16::from_le_bytes([pts[2], pts[3]]);
    for i in 1..n {
        let x = i16::from_le_bytes([pts[i*4], pts[i*4+1]]);
        let y = i16::from_le_bytes([pts[i*4+2], pts[i*4+3]]);
        render::line_window(&mut state.fb, abs, clip, prev_x, prev_y, x, y, fg);
        prev_x = x; prev_y = y;
    }
    log::rep(c.id, seq, format_args!("PolyLine wid={wid:#x} n={n}"), &[]);
    true
}

// PolySegment (66) — list of (x1,y1,x2,y2) 8-byte entries
fn handle_poly_segment(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 66, 0, "PolySegment short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let (fg, clip, abs) = match fetch_draw_context(c, state, seq, wid, gid, 66) {
        Some(v) => v,
        None => return true,
    };
    let segs = &raw[12..];
    let n = segs.len() / 8;
    for i in 0..n {
        let x1 = i16::from_le_bytes([segs[i*8],   segs[i*8+1]]);
        let y1 = i16::from_le_bytes([segs[i*8+2], segs[i*8+3]]);
        let x2 = i16::from_le_bytes([segs[i*8+4], segs[i*8+5]]);
        let y2 = i16::from_le_bytes([segs[i*8+6], segs[i*8+7]]);
        render::line_window(&mut state.fb, abs, clip, x1, y1, x2, y2, fg);
    }
    log::rep(c.id, seq, format_args!("PolySegment wid={wid:#x} n={n}"), &[]);
    true
}

// PolyRectangle (67) — outline rectangles
fn handle_poly_rectangle(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 67, 0, "PolyRectangle short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let (fg, clip, abs) = match fetch_draw_context(c, state, seq, wid, gid, 67) {
        Some(v) => v,
        None => return true,
    };
    let rects = &raw[12..];
    let n = rects.len() / 8;
    for i in 0..n {
        let x = i16::from_le_bytes([rects[i*8],   rects[i*8+1]]);
        let y = i16::from_le_bytes([rects[i*8+2], rects[i*8+3]]);
        let w = u16::from_le_bytes([rects[i*8+4], rects[i*8+5]]);
        let h = u16::from_le_bytes([rects[i*8+6], rects[i*8+7]]);
        render::rectangle_outline_window(&mut state.fb, abs, clip, x, y, w, h, fg);
    }
    log::rep(c.id, seq, format_args!("PolyRectangle wid={wid:#x} n={n}"), &[]);
    true
}

// PolyFillRectangle (70)
fn handle_poly_fill_rectangle(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 70, 0, "PolyFillRectangle short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let (fg, clip, abs) = match fetch_draw_context(c, state, seq, wid, gid, 70) {
        Some(v) => v,
        None => return true,
    };
    let rects = &raw[12..];
    let n = rects.len() / 8;
    for i in 0..n {
        let x = i16::from_le_bytes([rects[i*8],   rects[i*8+1]]);
        let y = i16::from_le_bytes([rects[i*8+2], rects[i*8+3]]);
        let w = u16::from_le_bytes([rects[i*8+4], rects[i*8+5]]);
        let h = u16::from_le_bytes([rects[i*8+6], rects[i*8+7]]);
        fill_rect_drawable(state, wid, abs, clip, x, y, w, h, fg);
    }
    log::rep(c.id, seq, format_args!("PolyFillRectangle did={wid:#x} n={n}"), &[]);
    true
}

// PutImage (72) — only ZPixmap (format 2) at depth 24 is supported.
// Works on any drawable (window or pixmap).
fn handle_put_image(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 24 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 72, 0, "PutImage short");
        return true;
    }
    let format = hdr.data;  // 0 = Bitmap, 1 = XYPixmap, 2 = ZPixmap
    let did  = get_u32(raw, 4);
    let _gid = get_u32(raw, 8);
    let w    = get_u16(raw, 12);
    let h    = get_u16(raw, 14);
    let dx   = get_u16(raw, 16) as i16;
    let dy   = get_u16(raw, 18) as i16;
    let left_pad = raw[20] as usize;
    let depth    = raw[21];
    let data = &raw[24..];
    if !is_drawable(state, did) {
        send_error(c, errcode::BAD_DRAWABLE, seq, did, 72, 0, "PutImage bad drawable");
        return true;
    }
    let n_pixels = w as usize * h as usize;
    let mut buf: Vec<u32> = vec![0u32; n_pixels];

    // Three X11 PutImage formats:
    //
    //   format=0 (Bitmap / XYBitmap):
    //       depth is always 1; one plane of bits.  Each row is
    //       padded to (scanline_pad / 8) bytes — our setup says
    //       scanline_pad=32, so rows are 4-byte aligned.  Within a
    //       byte, bit 0 is LEFTMOST pixel (little-endian bit order).
    //       Output values are 0 or 0xFFFFFFFF (the client supplies
    //       the foreground in the GC, but for our simple mask path
    //       we stash the raw bit).
    //
    //   format=1 (XYPixmap):
    //       Bit-planes, one after the other.  Phase 12 does not
    //       honor the per-plane layout — we just treat the first
    //       plane as the pixel value and stop.  Enough for xterm's
    //       cursor-mask usage.
    //
    //   format=2 (ZPixmap):
    //       Per-pixel bytes.  For depth 24/32 each pixel is 4
    //       bytes; for depth 8 each is 1 byte; etc.  Rows padded
    //       to 4 bytes.
    match format {
        0 | 1 => {
            // Bitmap / XYPixmap: 1 bit per pixel, first plane only.
            let row_bytes = ((w as usize + left_pad + 31) / 32) * 4;
            if data.len() < row_bytes * h as usize {
                send_error(c, errcode::BAD_LENGTH, seq, 0, 72, 0,
                           "PutImage XYBitmap data short");
                return true;
            }
            for row in 0..h as usize {
                for col in 0..w as usize {
                    let bit_idx = left_pad + col;
                    let byte = data[row * row_bytes + bit_idx / 8];
                    let bit = (byte >> (bit_idx & 7)) & 1;
                    buf[row * w as usize + col] =
                        if bit != 0 { 0xFFFFFFFF } else { 0 };
                }
            }
        }
        2 => {
            // ZPixmap.  Rows padded to 4 bytes; per-pixel size is
            // bits_per_pixel (looked up from depth below).
            let bpp: usize = match depth {
                1        => 1,  // stored row-packed
                4 | 8    => 8,
                16       => 16,
                24 | 32  => 32,
                _        => {
                    log::warn(format_args!(
                        "PutImage ZPixmap unsupported depth={depth}"
                    ));
                    return true;
                }
            };
            let row_bytes_unpadded = (w as usize * bpp + 7) / 8;
            let row_bytes = (row_bytes_unpadded + 3) & !3;
            if data.len() < row_bytes * h as usize {
                send_error(c, errcode::BAD_LENGTH, seq, 0, 72, 0,
                           "PutImage ZPixmap data short");
                return true;
            }
            match bpp {
                8 => for row in 0..h as usize {
                    for col in 0..w as usize {
                        let v = data[row * row_bytes + col] as u32;
                        buf[row * w as usize + col] = v | (v << 8) | (v << 16);
                    }
                },
                16 => for row in 0..h as usize {
                    for col in 0..w as usize {
                        let off = row * row_bytes + col * 2;
                        let v = u16::from_le_bytes([data[off], data[off+1]]) as u32;
                        buf[row * w as usize + col] = v;
                    }
                },
                32 => for row in 0..h as usize {
                    for col in 0..w as usize {
                        let off = row * row_bytes + col * 4;
                        buf[row * w as usize + col] = u32::from_le_bytes([
                            data[off], data[off+1], data[off+2], data[off+3],
                        ]);
                    }
                },
                1 => for row in 0..h as usize {
                    for col in 0..w as usize {
                        let byte = data[row * row_bytes + col / 8];
                        let bit = (byte >> (col & 7)) & 1;
                        buf[row * w as usize + col] =
                            if bit != 0 { 0xFFFFFFFF } else { 0 };
                    }
                },
                _ => {}
            }
        }
        _ => {
            log::warn(format_args!("PutImage unknown format={format}"));
            return true;
        }
    }

    write_rect(state, did, dx as i32, dy as i32, w, h, &buf);
    log::rep(c.id, seq, format_args!(
        "PutImage did={did:#x} fmt={format} depth={depth} {w}x{h} +{dx}+{dy}"
    ), &[]);
    true
}

// GetImage (73)
//     byte  1: format (1=XYPixmap, 2=ZPixmap)
//     dword 4: drawable
//     word 8:  x   word 10: y
//     word 12: width  word 14: height
//     dword 16: plane-mask
//
// Reply (32-byte header + data):
//     byte 1: depth
//     dword 4: visual (0 for pixmap)
//     + LISTofBYTE pixel data, padded to 4 bytes
fn handle_get_image(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 20 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 73, 0, "GetImage short");
        return true;
    }
    let format = hdr.data;
    let did = get_u32(raw, 4);
    let x = get_u16(raw, 8) as i16;
    let y = get_u16(raw, 10) as i16;
    let w = get_u16(raw, 12);
    let h = get_u16(raw, 14);
    let _plane_mask = get_u32(raw, 16);
    if format != 2 {
        send_error(c, errcode::BAD_VALUE, seq, format as u32, 73, 0, "GetImage only ZPixmap");
        return true;
    }
    if !is_drawable(state, did) {
        send_error(c, errcode::BAD_DRAWABLE, seq, did, 73, 0, "GetImage bad drawable");
        return true;
    }
    let depth: u8 = if let Some(p) = state.resources.pixmap(did) {
        p.depth
    } else {
        24
    };
    let staged = read_rect(state, did, x as i32, y as i32, w, h);

    // Pack pixels as ZPixmap bytes (4 bytes per pixel, little-endian).
    // Reply payload is padded up to a 4-byte boundary (already is for
    // 4-byte pixels, but compute defensively).
    let byte_count = w as usize * h as usize * 4;
    let total = pad4(byte_count);
    let extra_words = (total / 4) as u32;

    let mut out = Vec::with_capacity(32 + total);
    put_reply_header(&mut out, seq, depth, extra_words);
    // Body: visual (4), pad (20)
    put_u32(&mut out, 0);
    for _ in 0..20 { out.push(0); }
    debug_assert_eq!(out.len(), 32);
    for p in &staged {
        out.extend_from_slice(&p.to_le_bytes());
    }
    while out.len() < 32 + total { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetImage did={did:#x} {w}x{h}+{x}+{y} depth={depth}"), &out);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Pixmaps (Phase 5)
// ═════════════════════════════════════════════════════════════════════

// CreatePixmap (53)
//     byte 1: depth
//     dword 4: pid (new pixmap id)
//     dword 8: drawable (parent, for depth/visual inheritance)
//     word 12: width  word 14: height
fn handle_create_pixmap(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 53, 0, "CreatePixmap short");
        return true;
    }
    let depth = hdr.data;
    let pid = get_u32(raw, 4);
    let parent = get_u32(raw, 8);
    let w = get_u16(raw, 12);
    let h = get_u16(raw, 14);
    if !belongs_to_client(c.id, pid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, pid, 53, 0, "CreatePixmap id not in client range");
        return true;
    }
    if state.resources.get(pid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, pid, 53, 0, "CreatePixmap id in use");
        return true;
    }
    if !is_drawable(state, parent) {
        send_error(c, errcode::BAD_DRAWABLE, seq, parent, 53, 0, "CreatePixmap bad drawable");
        return true;
    }
    if w == 0 || h == 0 {
        send_error(c, errcode::BAD_VALUE, seq, 0, 53, 0, "CreatePixmap zero size");
        return true;
    }
    // Accept any depth the server advertises in the connection
    // setup (1, 4, 8, 24, 32 in practice).  Pixmap storage is
    // always u32-per-pixel; the depth is used by RENDER and
    // CopyPlane to decide interpretation.
    if !matches!(depth, 1 | 4 | 8 | 16 | 24 | 32) {
        send_error(c, errcode::BAD_MATCH, seq, depth as u32, 53, 0,
                   "CreatePixmap unsupported depth");
        return true;
    }
    state.resources.insert(pid, c.id, Resource::Pixmap(Pixmap::new(w, h, depth)));
    log::rep(c.id, seq, format_args!("CreatePixmap pid={pid:#x} {w}x{h} depth={depth}"), &[]);
    true
}

// FreePixmap (54)
fn handle_free_pixmap(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 54, 0, "FreePixmap short");
        return true;
    }
    let pid = get_u32(raw, 4);
    if state.resources.pixmap(pid).is_none() {
        send_error(c, errcode::BAD_PIXMAP, seq, pid, 54, 0, "FreePixmap bad pid");
        return true;
    }
    state.resources.remove(pid);
    log::rep(c.id, seq, format_args!("FreePixmap {pid:#x}"), &[]);
    true
}

/// Any valid drawable id (root, window, or pixmap).  Used by opcodes
/// that accept either windows or pixmaps (CreateGC, PutImage, CopyArea,
/// GetImage, CreatePixmap's parent).
fn is_drawable(state: &ServerState, id: u32) -> bool {
    if id == setup::ROOT_WINDOW_ID { return true; }
    if state.resources.window(id).is_some() { return true; }
    if state.resources.pixmap(id).is_some() { return true; }
    false
}

// ═════════════════════════════════════════════════════════════════════
// Colormaps (Phase 4 — trivial TrueColor)
// ═════════════════════════════════════════════════════════════════════

// CreateColormap (78)
//     byte 1: alloc (0=None, 1=All)
//     dword 4: mid (new colormap id)
//     dword 8: window
//     dword 12: visual
fn handle_create_colormap(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 78, 0, "CreateColormap short");
        return true;
    }
    let mid = get_u32(raw, 4);
    let _win = get_u32(raw, 8);
    let visual = get_u32(raw, 12);
    state.resources.insert(mid, c.id, Resource::Colormap { visual });
    log::rep(c.id, seq, format_args!("CreateColormap mid={mid:#x} visual={visual:#x}"), &[]);
    true
}

// FreeColormap (79)
fn handle_free_colormap(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 79, 0, "FreeColormap short");
        return true;
    }
    let mid = get_u32(raw, 4);
    state.resources.remove(mid);
    log::rep(c.id, seq, format_args!("FreeColormap mid={mid:#x}"), &[]);
    true
}

// AllocColor (84)
//     dword 4: cmap
//     word 8: red  word 10: green  word 12: blue
fn handle_alloc_color(c: &mut Client, _state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 84, 0, "AllocColor short");
        return true;
    }
    let _cmap = get_u32(raw, 4);
    let r = get_u16(raw, 8);
    let g = get_u16(raw, 10);
    let b = get_u16(raw, 12);
    let pixel = colormap::pack_pixel(r, g, b);
    let r_act = colormap::round_channel_16(r);
    let g_act = colormap::round_channel_16(g);
    let b_act = colormap::round_channel_16(b);
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u16(&mut out, r_act);
    put_u16(&mut out, g_act);
    put_u16(&mut out, b_act);
    put_u16(&mut out, 0);          // pad
    put_u32(&mut out, pixel);      // pixel
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("AllocColor ({r:04x},{g:04x},{b:04x}) → pixel={pixel:#x}"), &out);
    true
}

// AllocNamedColor (85)
//     dword 4: cmap
//     word 8: name-length  word 10: unused
//     bytes 12+: STRING8 name + pad
//
// Reply (32 bytes):
//     word 8: exact_red  word 10: exact_green  word 12: exact_blue
//     word 14: visual_red ... word 18: visual_blue
//     dword 20: pixel (actually dword 16 per spec; check again)
// Actual reply layout (X11 spec):
//     dword 8:  pixel
//     word 12:  exact_red  word 14: exact_green  word 16: exact_blue
//     word 18:  visual_red word 20: visual_green word 22: visual_blue
//     10 bytes pad
fn handle_alloc_named_color(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 85, 0, "AllocNamedColor short");
        return true;
    }
    let _cmap = get_u32(raw, 4);
    let nlen = get_u16(raw, 8) as usize;
    if raw.len() < 12 + nlen {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 85, 0, "AllocNamedColor name overflow");
        return true;
    }
    let name_bytes = &raw[12..12 + nlen];
    let name = match core::str::from_utf8(name_bytes) {
        Ok(s) => s,
        Err(_) => {
            send_error(c, errcode::BAD_VALUE, seq, 0, 85, 0, "AllocNamedColor non-utf8");
            return true;
        }
    };
    let (r, g, b) = match colormap::lookup_named(name) {
        Some(rgb) => rgb,
        None => {
            send_error(c, errcode::BAD_NAME, seq, 0, 85, 0, "AllocNamedColor unknown color");
            return true;
        }
    };
    let pixel = colormap::pack_pixel(r, g, b);
    let rr = colormap::round_channel_16(r);
    let gg = colormap::round_channel_16(g);
    let bb = colormap::round_channel_16(b);
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u32(&mut out, pixel);         // 8..12
    put_u16(&mut out, r); put_u16(&mut out, g); put_u16(&mut out, b);        // exact 12..18
    put_u16(&mut out, rr); put_u16(&mut out, gg); put_u16(&mut out, bb);     // visual 18..24
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("AllocNamedColor {name:?} → pixel={pixel:#x}"), &out);
    true
}

// FreeColors (88) — stub (no-op on TrueColor)
fn handle_free_colors_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("FreeColors (stub)"), &[]);
    true
}

// QueryColors (91)
//     dword 4: cmap
//     dword 8..: list of pixel values (4 bytes each)
//
// Reply:
//     dword 8: length = n
//     word 12: pad
//     LISTofRGB: 8 bytes each (word red, word green, word blue, word pad)
fn handle_query_colors(c: &mut Client, _state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 91, 0, "QueryColors short");
        return true;
    }
    let _cmap = get_u32(raw, 4);
    let pixels_bytes = &raw[8..];
    let n = pixels_bytes.len() / 4;
    let extra_words = (2 * n) as u32; // each RGB entry is 8 bytes = 2 words
    let mut out = Vec::with_capacity(32 + 8 * n);
    put_reply_header(&mut out, seq, 0, extra_words);
    put_u16(&mut out, n as u16);
    for _ in 0..22 { out.push(0); }
    debug_assert_eq!(out.len(), 32);
    for i in 0..n {
        let p = u32::from_le_bytes([
            pixels_bytes[i*4], pixels_bytes[i*4+1],
            pixels_bytes[i*4+2], pixels_bytes[i*4+3],
        ]);
        let r = (((p >> 16) & 0xFF) as u16) << 8;
        let g = (((p >> 8)  & 0xFF) as u16) << 8;
        let b = ((p & 0xFF) as u16) << 8;
        put_u16(&mut out, r | (r >> 8));
        put_u16(&mut out, g | (g >> 8));
        put_u16(&mut out, b | (b >> 8));
        put_u16(&mut out, 0);
    }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("QueryColors n={n}"), &out);
    true
}

// LookupColor (92) — like AllocNamedColor but does not allocate
fn handle_lookup_color(c: &mut Client, _state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 92, 0, "LookupColor short");
        return true;
    }
    let _cmap = get_u32(raw, 4);
    let nlen = get_u16(raw, 8) as usize;
    if raw.len() < 12 + nlen {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 92, 0, "LookupColor name overflow");
        return true;
    }
    let name = match core::str::from_utf8(&raw[12..12 + nlen]) {
        Ok(s) => s,
        Err(_) => {
            send_error(c, errcode::BAD_VALUE, seq, 0, 92, 0, "LookupColor non-utf8");
            return true;
        }
    };
    let (r, g, b) = match colormap::lookup_named(name) {
        Some(rgb) => rgb,
        None => {
            send_error(c, errcode::BAD_NAME, seq, 0, 92, 0, "LookupColor unknown");
            return true;
        }
    };
    let rr = colormap::round_channel_16(r);
    let gg = colormap::round_channel_16(g);
    let bb = colormap::round_channel_16(b);
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u16(&mut out, r); put_u16(&mut out, g); put_u16(&mut out, b);   // exact
    put_u16(&mut out, rr); put_u16(&mut out, gg); put_u16(&mut out, bb); // visual
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("LookupColor {name:?}"), &out);
    true
}

// ─────────────────────────────────────────────────────────────────────
// Drawing helper: fetch (foreground, clip, absolute origin) for a
// (window, gc) pair.  Sends a BadDrawable/BadGContext error and returns
// None if either resource is missing.
// ─────────────────────────────────────────────────────────────────────

fn fetch_draw_context(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    wid: u32,
    gid: u32,
    major: u8,
) -> Option<(u32, ClipRect, (i32, i32))> {
    if !is_drawable(state, wid) {
        send_error(c, errcode::BAD_DRAWABLE, seq, wid, major, 0, "drawable not a window or pixmap");
        return None;
    }
    let fg = match state.resources.gc(gid) {
        Some(g) => g.foreground,
        None => {
            send_error(c, errcode::BAD_GCONTEXT, seq, gid, major, 0, "bad gc");
            return None;
        }
    };
    // For pixmaps, abs is (0,0) and clip is the pixmap bounds.  For
    // windows, abs is screen-relative origin and clip is the
    // window's screen-rect.  Drawing helpers below dispatch on
    // whether the drawable is a window or a pixmap.
    let (clip, abs) = if let Some(p) = state.resources.pixmap(wid) {
        (ClipRect::new(0, 0, p.width as i32, p.height as i32), (0, 0))
    } else {
        (window_clip(state, wid).unwrap(), abs_origin(state, wid))
    };
    Some((fg, clip, abs))
}

/// Draw a filled rectangle into any drawable.  Window-targeted
/// fills go through the framebuffer compositor; pixmap-targeted
/// fills land directly in the owned pixel buffer.
fn fill_rect_drawable(
    state: &mut ServerState,
    did: u32,
    abs: (i32, i32),
    clip: ClipRect,
    x: i16, y: i16, w: u16, h: u16,
    pixel: u32,
) {
    if state.resources.pixmap(did).is_some() {
        let p = state.resources.pixmap_mut(did).unwrap();
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                p.put(x as i32 + col, y as i32 + row, pixel);
            }
        }
        return;
    }
    render::fill_rect_window(&mut state.fb, abs, clip, x, y, w, h, pixel);
}

/// Same idea for a single-pixel write.  Used by PolyPoint /
/// PolyLine / PolyRectangle / etc.
#[allow(dead_code)]
fn put_pixel_drawable(
    state: &mut ServerState,
    did: u32,
    _abs: (i32, i32),
    clip: ClipRect,
    px: i32, py: i32, pixel: u32,
) {
    if state.resources.pixmap(did).is_some() {
        let p = state.resources.pixmap_mut(did).unwrap();
        p.put(px, py, pixel);
        return;
    }
    render::put_pixel_clipped(&mut state.fb, clip, px, py, pixel);
}

/// RENDER destination metadata, captured up front so the per-pixel
/// hot loop doesn't fight the borrow checker over `state.resources`.
#[derive(Debug, Clone)]
struct RenderDst {
    /// Drawable id (window or pixmap).  Picture drawable_id == 0
    /// (solid-fill picture) is never a valid destination.
    pub did: u32,
    /// True if `did` is a pixmap, false if it's a window.
    pub is_pixmap: bool,
    /// For windows: framebuffer-absolute origin and clip rect.
    /// For pixmaps: (0,0) and a clip covering the pixmap's bounds.
    pub abs: (i32, i32),
    pub clip: ClipRect,
    /// XFIXES picture clip region in destination-local coordinates.
    pub clip_region: Option<Region>,
}

fn fetch_render_dst(state: &ServerState, pid: u32) -> Option<RenderDst> {
    let p = state.resources.picture(pid)?;
    if p.drawable == 0 { return None; }
    let did = p.drawable;
    let clip_region = p.clip_region.clone();
    if let Some(pixmap) = state.resources.pixmap(did) {
        return Some(RenderDst {
            did, is_pixmap: true,
            abs: (0, 0),
            clip: ClipRect::new(0, 0, pixmap.width as i32, pixmap.height as i32),
            clip_region,
        });
    }
    if state.resources.window(did).is_some() {
        return Some(RenderDst {
            did, is_pixmap: false,
            abs: abs_origin(state, did),
            clip: window_clip(state, did).unwrap_or(ClipRect::new(0,0,0,0)),
            clip_region,
        });
    }
    None
}

/// Read the current value of a destination pixel.  Used by Over.
fn render_read_pixel(state: &ServerState, dst: &RenderDst, lx: i32, ly: i32) -> u32 {
    if dst.is_pixmap {
        let p = state.resources.pixmap(dst.did).unwrap();
        return p.get(lx, ly) | 0xFF000000;
    }
    let px = dst.abs.0 + lx;
    let py = dst.abs.1 + ly;
    if px < 0 || py < 0
        || px >= state.fb.width as i32 || py >= state.fb.height as i32 {
        return 0xFF000000;
    }
    let stride = state.fb.stride_px;
    state.fb.pixels_read()[py as usize * stride + px as usize] | 0xFF000000
}

/// Write a final composited pixel to the destination, respecting
/// every clip layer.  `lx`/`ly` are destination-local coordinates.
fn render_write_pixel(state: &mut ServerState, dst: &RenderDst, lx: i32, ly: i32, pixel: u32) {
    if let Some(ref rg) = dst.clip_region {
        if !rg.contains(lx, ly) { return; }
    }
    if dst.is_pixmap {
        let p = state.resources.pixmap_mut(dst.did).unwrap();
        p.put(lx, ly, pixel & 0x00FFFFFF);
        return;
    }
    let px = dst.abs.0 + lx;
    let py = dst.abs.1 + ly;
    if !dst.clip.contains(px, py) { return; }
    if px < 0 || py < 0
        || px >= state.fb.width as i32 || py >= state.fb.height as i32 {
        return;
    }
    let stride = state.fb.stride_px;
    let pix = state.fb.pixels_mut();
    pix[py as usize * stride + px as usize] = pixel & 0x00FFFFFF;
}

// ═════════════════════════════════════════════════════════════════════
// Phase 6 — Fonts and text
// ═════════════════════════════════════════════════════════════════════

// OpenFont (45)
//     dword 4: fid
//     word 8:  name length  word 10: unused
//     bytes 12+: STRING8 name (+ pad4)
fn handle_open_font(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 45, 0, "OpenFont short");
        return true;
    }
    let fid = get_u32(raw, 4);
    let nlen = get_u16(raw, 8) as usize;
    if raw.len() < 12 + nlen {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 45, 0, "OpenFont name overflow");
        return true;
    }
    let name = core::str::from_utf8(&raw[12..12 + nlen]).unwrap_or("<invalid>");
    if !belongs_to_client(c.id, fid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, fid, 45, 0, "OpenFont id not in client range");
        return true;
    }
    if state.resources.get(fid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, fid, 45, 0, "OpenFont id in use");
        return true;
    }
    state.resources.insert(fid, c.id, Resource::Font(FontRef::embedded(name)));
    log::rep(c.id, seq, format_args!("OpenFont fid={fid:#x} name={name:?}"), &[]);
    true
}

// CloseFont (46)
fn handle_close_font(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 46, 0, "CloseFont short");
        return true;
    }
    let fid = get_u32(raw, 4);
    if state.resources.font(fid).is_none() {
        send_error(c, errcode::BAD_FONT, seq, fid, 46, 0, "CloseFont bad fid");
        return true;
    }
    state.resources.remove(fid);
    log::rep(c.id, seq, format_args!("CloseFont {fid:#x}"), &[]);
    true
}

/// Resolve a font-or-gcontext id (QueryFont and QueryTextExtents accept
/// either).  When the id names a GC we return the font id bound to it.
fn resolve_fontable<'a>(state: &'a ServerState, id: u32) -> Option<&'a FontRef> {
    if let Some(f) = state.resources.font(id) {
        return Some(f);
    }
    if let Some(g) = state.resources.gc(id) {
        if g.font != 0 {
            return state.resources.font(g.font);
        }
        // GC without an explicit font — fall back to the built-in metrics
        // via a dummy FontRef stored on a static.  Here we just return
        // None and let the caller synthesize the stock reply.
    }
    None
}

/// The server-wide built-in font reference used when a client queries
/// a GC that has no font attached.  Name is deliberately generic so
/// logs still identify the source.
fn stock_font() -> FontRef {
    FontRef::embedded("<stock>")
}

// QueryFont (47)
//
// Reply (32-byte header + body):
//     byte 1:  pad
//     word 2:  seq
//     dword 4: reply-length (extra 4-byte words)
//     CHARINFO min-bounds (12 bytes)
//     pad (4)
//     CHARINFO max-bounds (12 bytes)
//     word    min-char-or-byte2
//     word    max-char-or-byte2
//     word    default-char
//     word    n-props
//     byte    draw-direction (0=LtoR)
//     byte    min-byte1
//     byte    max-byte1
//     byte    all-chars-exist
//     word    font-ascent
//     word    font-descent
//     dword   n-chars
//     LISTofFONTPROP (n-props * 8 bytes)  — we send 0
//     LISTofCHARINFO (n-chars * 12 bytes)
//
// CHARINFO layout (12 bytes):
//     word left-side-bearing
//     word right-side-bearing
//     word character-width
//     word ascent
//     word descent
//     word attributes
fn handle_query_font(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 47, 0, "QueryFont short");
        return true;
    }
    let id = get_u32(raw, 4);
    let font_owned: FontRef;
    let f: &FontRef = if let Some(f) = resolve_fontable(state, id) {
        f
    } else if state.resources.get(id).is_some() {
        // GC without a font — use stock.
        font_owned = stock_font();
        &font_owned
    } else {
        send_error(c, errcode::BAD_FONT, seq, id, 47, 0, "QueryFont bad id");
        return true;
    };

    let first = f.first;
    let last  = f.last;
    let n_chars = (last - first + 1) as u32;
    let n_props: u16 = 0;

    // Build the reply into a buffer, patch the length field at the end.
    // The X11 QueryFont reply interleaves structured fields across the
    // fixed 32-byte header boundary, so hand-computing extra_words is
    // error-prone.  Patching post hoc is simpler and guaranteed correct.
    let mut out = Vec::with_capacity(32 + 52 + n_chars as usize * 12);
    put_reply_header(&mut out, seq, 0, 0);         // length patched below
    // min-bounds CHARINFO: all zeros
    for _ in 0..12 { out.push(0); }
    // 4 bytes pad
    for _ in 0..4 { out.push(0); }
    // max-bounds CHARINFO — full box
    put_u16(&mut out, 0);                   // lsb
    put_u16(&mut out, f.char_w);            // rsb
    put_u16(&mut out, f.char_w);            // width
    put_u16(&mut out, f.ascent as u16);     // ascent
    put_u16(&mut out, f.descent as u16);    // descent
    put_u16(&mut out, 0);                   // attributes
    // 4 bytes pad after max-bounds
    for _ in 0..4 { out.push(0); }
    put_u16(&mut out, first);               // min-char
    put_u16(&mut out, last);                // max-char
    put_u16(&mut out, b' ' as u16);         // default-char
    put_u16(&mut out, n_props);             // n-props
    put_u8(&mut out, 0);                    // draw-direction LtoR
    put_u8(&mut out, 0);                    // min-byte1
    put_u8(&mut out, 0);                    // max-byte1
    put_u8(&mut out, 1);                    // all-chars-exist
    put_u16(&mut out, f.ascent as u16);     // font-ascent
    put_u16(&mut out, f.descent as u16);    // font-descent
    put_u32(&mut out, n_chars);

    // Per-char CHARINFO entries (uniform metrics — our font is monospace).
    for _ in 0..n_chars {
        put_u16(&mut out, 0);                   // lsb
        put_u16(&mut out, f.char_w);            // rsb
        put_u16(&mut out, f.char_w);            // width
        put_u16(&mut out, f.ascent as u16);     // ascent
        put_u16(&mut out, f.descent as u16);    // descent
        put_u16(&mut out, 0);                   // attributes
    }

    // Patch extra_words into the reply_length slot (bytes 4..8).  The
    // total length must be a multiple of 4.
    assert!(out.len() % 4 == 0);
    assert!(out.len() >= 32);
    let extra_words = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra_words.to_le_bytes());

    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("QueryFont id={id:#x} chars={n_chars}"), &[]);
    true
}

// QueryTextExtents (48)
//     byte 1:  odd-length (if low bit of string_len)
//     dword 4: font-or-gcontext
//     bytes 8+: STRING16 (two bytes per char)
//
// Reply body (not including 32-byte header):
//     byte  1: draw-direction
//     word  2: seq
//     dword 4: reply-length (0)
//     word    font-ascent
//     word    font-descent
//     word    overall-ascent
//     word    overall-descent
//     dword   overall-width
//     dword   overall-left
//     dword   overall-right
//     4 bytes pad
fn handle_query_text_extents(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 48, 0, "QueryTextExtents short");
        return true;
    }
    let odd_len = hdr.data != 0;
    let id = get_u32(raw, 4);
    let mut s16 = raw[8..].to_vec();
    // STRING16 is zero-padded to 4-byte alignment.  If the low bit of
    // the character count was set (odd_len flag), the final 2 bytes
    // are padding.
    if odd_len && s16.len() >= 2 { s16.truncate(s16.len() - 2); }
    // Convert STRING16 chars to bytes (high-byte is 0 for our ASCII font).
    let nchars = s16.len() / 2;
    let mut bytes = Vec::with_capacity(nchars);
    for i in 0..nchars {
        // X11 CHAR2B is big-endian: byte1, byte2.  We only honor byte2.
        bytes.push(s16[i*2 + 1]);
    }

    let font_owned: FontRef;
    let f: &FontRef = if let Some(f) = resolve_fontable(state, id) {
        f
    } else {
        font_owned = stock_font();
        &font_owned
    };

    let width = font::text_width(&bytes);
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u16(&mut out, f.ascent as u16);        // font-ascent
    put_u16(&mut out, f.descent as u16);       // font-descent
    put_u16(&mut out, f.ascent as u16);        // overall-ascent
    put_u16(&mut out, f.descent as u16);       // overall-descent
    put_u32(&mut out, width as u32);           // overall-width
    put_u32(&mut out, 0);                      // overall-left
    put_u32(&mut out, width as u32);           // overall-right
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("QueryTextExtents id={id:#x} n={nchars} w={width}"), &[]);
    true
}

// ListFonts (49) — always returns the same three names.
fn handle_list_fonts(c: &mut Client, _state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 49, 0, "ListFonts short");
        return true;
    }
    let _max = get_u16(raw, 4);
    // We ignore the pattern — return a fixed stock list.
    let names: &[&str] = &[
        "fixed",
        "-misc-fixed-medium-r-normal--13-120-75-75-c-80-iso8859-1",
        "cursor",
    ];
    let mut body = Vec::new();
    for n in names {
        let len = n.len() as u8;
        body.push(len);
        body.extend_from_slice(n.as_bytes());
    }
    while body.len() % 4 != 0 { body.push(0); }
    let extra_words = (body.len() / 4) as u32;
    let mut out = Vec::with_capacity(32 + body.len());
    put_reply_header(&mut out, seq, 0, extra_words);
    put_u16(&mut out, names.len() as u16);
    for _ in 0..22 { out.push(0); }
    debug_assert_eq!(out.len(), 32);
    out.extend_from_slice(&body);
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("ListFonts → {} names", names.len()), &[]);
    true
}

// ListFontsWithInfo (50) — we're supposed to send one reply per font
// and then a terminator.  We cheat: send only the terminator.
fn handle_list_fonts_with_info(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _raw: &[u8],
) -> bool {
    // Terminator: reply with name-length = 0 and everything else zero.
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 7);   // 7 extra words = match dummy body
    for _ in 0..(32 + 7*4 - 8) { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("ListFontsWithInfo → terminator only"), &[]);
    true
}

// SetFontPath (51) — accept and ignore.
fn handle_set_font_path_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("SetFontPath (stub)"), &[]);
    true
}

// GetFontPath (52) — empty list.
fn handle_get_font_path_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u16(&mut out, 0);
    for _ in 0..22 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetFontPath → (empty)"), &[]);
    true
}

// ─── Text drawing ────────────────────────────────────────────────────

fn fetch_text_context(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    wid: u32,
    gid: u32,
    major: u8,
) -> Option<(u32, u32, ClipRect, (i32, i32))> {
    if state.resources.window(wid).is_none() {
        send_error(c, errcode::BAD_DRAWABLE, seq, wid, major, 0, "text drawable");
        return None;
    }
    let (fg, bg) = match state.resources.gc(gid) {
        Some(g) => (g.foreground, g.background),
        None => {
            send_error(c, errcode::BAD_GCONTEXT, seq, gid, major, 0, "text gc");
            return None;
        }
    };
    let clip = window_clip(state, wid).unwrap();
    let abs = abs_origin(state, wid);
    Some((fg, bg, clip, abs))
}

// ImageText8 (76)
//     byte 1: string length n
//     dword 4: drawable
//     dword 8: gc
//     word 12: x   word 14: y
//     bytes 16..16+n: STRING8
fn handle_image_text_8(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 76, 0, "ImageText8 short");
        return true;
    }
    let n = hdr.data as usize;
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let x = get_u16(raw, 12) as i16;
    let y = get_u16(raw, 14) as i16;
    if raw.len() < 16 + n {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 76, 0, "ImageText8 string overflow");
        return true;
    }
    let text = &raw[16..16 + n];
    let (fg, bg, clip, abs) = match fetch_text_context(c, state, seq, wid, gid, 76) {
        Some(v) => v,
        None => return true,
    };
    font::draw_text_window(&mut state.fb, abs, clip, x, y, text, fg, bg, true);
    log::rep(c.id, seq, format_args!("ImageText8 wid={wid:#x} n={n} @({x},{y})"), &[]);
    true
}

// ImageText16 (77)
fn handle_image_text_16(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 77, 0, "ImageText16 short");
        return true;
    }
    let n = hdr.data as usize;
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let x = get_u16(raw, 12) as i16;
    let y = get_u16(raw, 14) as i16;
    if raw.len() < 16 + 2 * n {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 77, 0, "ImageText16 string overflow");
        return true;
    }
    // Flatten STRING16 to byte2 only (we render ASCII glyphs).
    let mut bytes = Vec::with_capacity(n);
    for i in 0..n {
        bytes.push(raw[16 + 2*i + 1]);
    }
    let (fg, bg, clip, abs) = match fetch_text_context(c, state, seq, wid, gid, 77) {
        Some(v) => v,
        None => return true,
    };
    font::draw_text_window(&mut state.fb, abs, clip, x, y, &bytes, fg, bg, true);
    log::rep(c.id, seq, format_args!("ImageText16 wid={wid:#x} n={n} @({x},{y})"), &[]);
    true
}

// PolyText8 (74) — accept but treat as a single byte string.
// The real request has TEXTITEM8 entries: {delta, STRING8}+ with font
// switches.  We ignore deltas, extract the first string, and draw it.
fn handle_poly_text_8(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 74, 0, "PolyText8 short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let x = get_u16(raw, 12) as i16;
    let y = get_u16(raw, 14) as i16;
    // raw[16..] is TEXTITEM8 list.  First byte is length (or 255 for
    // font switch).  We walk naïvely and collect any byte runs.
    let mut items = &raw[16..];
    let mut cx = x;
    let (fg, bg, clip, abs) = match fetch_text_context(c, state, seq, wid, gid, 74) {
        Some(v) => v,
        None => return true,
    };
    while !items.is_empty() {
        let hdr = items[0];
        if hdr == 0 { break; }
        if hdr == 255 {
            // Font switch: next 4 bytes are a font id.  Skip.
            if items.len() < 5 { break; }
            items = &items[5..];
            continue;
        }
        let n = hdr as usize;
        if items.len() < 2 + n { break; }
        let _delta = items[1] as i8;
        let text = &items[2..2 + n];
        font::draw_text_window(&mut state.fb, abs, clip, cx, y, text, fg, bg, false);
        cx = cx.saturating_add(font::text_width(text) as i16);
        items = &items[2 + n..];
    }
    log::rep(c.id, seq, format_args!("PolyText8 wid={wid:#x}"), &[]);
    true
}

// PolyText16 (75) — same but 16-bit chars, byte2 only.
fn handle_poly_text_16(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 75, 0, "PolyText16 short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let gid = get_u32(raw, 8);
    let x = get_u16(raw, 12) as i16;
    let y = get_u16(raw, 14) as i16;
    let mut items = &raw[16..];
    let mut cx = x;
    let (fg, bg, clip, abs) = match fetch_text_context(c, state, seq, wid, gid, 75) {
        Some(v) => v,
        None => return true,
    };
    while !items.is_empty() {
        let hdr = items[0];
        if hdr == 0 { break; }
        if hdr == 255 {
            if items.len() < 5 { break; }
            items = &items[5..];
            continue;
        }
        let n = hdr as usize;
        if items.len() < 2 + 2 * n { break; }
        let _delta = items[1] as i8;
        let mut bytes = Vec::with_capacity(n);
        for i in 0..n { bytes.push(items[2 + 2*i + 1]); }
        font::draw_text_window(&mut state.fb, abs, clip, cx, y, &bytes, fg, bg, false);
        cx = cx.saturating_add(font::text_width(&bytes) as i16);
        items = &items[2 + 2*n..];
    }
    log::rep(c.id, seq, format_args!("PolyText16 wid={wid:#x}"), &[]);
    true
}

// CreateGlyphCursor (94) — stub.  Registers an opaque cursor id so
// subsequent CreateWindow calls with `cursor` attribute don't fail.
fn handle_create_glyph_cursor_stub(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 94, 0, "CreateGlyphCursor short");
        return true;
    }
    let cid = get_u32(raw, 4);
    if !belongs_to_client(c.id, cid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, cid, 94, 0, "CreateGlyphCursor id not in client range");
        return true;
    }
    // We don't actually store cursors — the id is just reserved so the
    // client doesn't see BadIdChoice on reuse.  Insert a dummy colormap
    // variant to occupy the slot; it will never be consulted as a
    // colormap because we never look for one at this id.
    state.resources.insert(cid, c.id, Resource::Colormap { visual: 0 });
    log::rep(c.id, seq, format_args!("CreateGlyphCursor {cid:#x} (stub)"), &[]);
    true
}

// QueryBestSize (97) — echo back whatever was requested.
//     byte 1: class
//     dword 4: drawable
//     word 8: width  word 10: height
fn handle_query_best_size(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 97, 0, "QueryBestSize short");
        return true;
    }
    let w = get_u16(raw, 8);
    let h = get_u16(raw, 10);
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u16(&mut out, w);
    put_u16(&mut out, h);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("QueryBestSize → {w}x{h}"), &[]);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Phase 7 — Input handlers
// ═════════════════════════════════════════════════════════════════════

// ── Grabs ───────────────────────────────────────────────────────────
// All grab implementations are bookkeeping stubs: we accept the
// request, log it, and reply GrabSuccess.  The actual "route all
// events to this client" semantics are a Phase 8 / twm follow-up.

// GrabPointer (26) — reply with status byte.
//     byte 1:  owner_events
//     dword 4: grab_window
//     word 8:  event_mask
//     byte 10: pointer_mode
//     byte 11: keyboard_mode
//     dword 12: confine_to (0 = None)
//     dword 16: cursor (0 = None)
//     dword 20: time
fn handle_grab_pointer(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 24 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 26, 0, "GrabPointer short");
        return true;
    }
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);  // byte1 = status (GrabSuccess = 0)
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GrabPointer → GrabSuccess"), &[]);
    true
}

fn handle_ungrab_pointer(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("UngrabPointer"), &[]);
    true
}

fn handle_grab_button_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("GrabButton (stub)"), &[]);
    true
}

fn handle_ungrab_button_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("UngrabButton (stub)"), &[]);
    true
}

// GrabKeyboard (31)
fn handle_grab_keyboard(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 31, 0, "GrabKeyboard short");
        return true;
    }
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GrabKeyboard → GrabSuccess"), &[]);
    true
}

fn handle_ungrab_keyboard(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("UngrabKeyboard"), &[]);
    true
}

fn handle_grab_key_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("GrabKey (stub)"), &[]);
    true
}

fn handle_ungrab_key_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("UngrabKey (stub)"), &[]);
    true
}

fn handle_allow_events_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("AllowEvents (stub)"), &[]);
    true
}

fn handle_grab_server_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("GrabServer (stub)"), &[]);
    true
}

fn handle_ungrab_server_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("UngrabServer (stub)"), &[]);
    true
}

// ── Pointer ─────────────────────────────────────────────────────────

// QueryPointer (38)
//     dword 4: window
//
// Reply (32 bytes):
//     byte 1: same_screen
//     dword 8:  root
//     dword 12: child
//     word 16:  root-x
//     word 18:  root-y
//     word 20:  win-x
//     word 22:  win-y
//     word 24:  mask
//     2 bytes unused
fn handle_query_pointer(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 38, 0, "QueryPointer short");
        return true;
    }
    let wid = get_u32(raw, 4);
    if state.resources.window(wid).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, wid, 38, 0, "QueryPointer bad window");
        return true;
    }
    let abs = abs_origin(state, wid);
    let p = &state.input;
    let win_x = (p.pointer_x as i32 - abs.0) as i16;
    let win_y = (p.pointer_y as i32 - abs.1) as i16;

    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 1, 0); // byte1 = same_screen
    put_u32(&mut out, setup::ROOT_WINDOW_ID);  // root
    put_u32(&mut out, 0);                      // child (None)
    put_u16(&mut out, p.pointer_x as u16);     // root_x
    put_u16(&mut out, p.pointer_y as u16);     // root_y
    put_u16(&mut out, win_x as u16);           // win_x
    put_u16(&mut out, win_y as u16);           // win_y
    put_u16(&mut out, p.mask);                 // mask
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "QueryPointer ({},{}) mask={:#x}", p.pointer_x, p.pointer_y, p.mask
    ), &[]);
    true
}

// TranslateCoordinates (40)
//     dword 4: src_window
//     dword 8: dst_window
//     word 12: src_x   word 14: src_y
//
// Reply (32 bytes):
//     byte 1: same_screen
//     dword 8:  child (0 = none)
//     word 12:  dst_x
//     word 14:  dst_y
//     16 pad
fn handle_translate_coordinates(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 40, 0, "TranslateCoordinates short");
        return true;
    }
    let src_wid = get_u32(raw, 4);
    let dst_wid = get_u32(raw, 8);
    let src_x = get_u16(raw, 12) as i16;
    let src_y = get_u16(raw, 14) as i16;
    if state.resources.window(src_wid).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, src_wid, 40, 0, "TranslateCoords bad src");
        return true;
    }
    if state.resources.window(dst_wid).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, dst_wid, 40, 0, "TranslateCoords bad dst");
        return true;
    }
    // Convert src-local → screen-absolute → dst-local.
    let src_abs = abs_origin(state, src_wid);
    let dst_abs = abs_origin(state, dst_wid);
    let abs_x = src_abs.0 + src_x as i32;
    let abs_y = src_abs.1 + src_y as i32;
    let dst_x = (abs_x - dst_abs.0) as i16;
    let dst_y = (abs_y - dst_abs.1) as i16;
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 1, 0); // same_screen = 1
    put_u32(&mut out, 0);                  // child = None
    put_u16(&mut out, dst_x as u16);
    put_u16(&mut out, dst_y as u16);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "TranslateCoordinates ({src_x},{src_y}) {src_wid:#x}→{dst_wid:#x} → ({dst_x},{dst_y})"
    ), &[]);
    true
}

// WarpPointer (41)
//     dword 4:  src-window
//     dword 8:  dst-window
//     word 12:  src-x  word 14: src-y
//     word 16:  src-w  word 18: src-h
//     word 20:  dst-x  word 22: dst-y
fn handle_warp_pointer(c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 24 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 41, 0, "WarpPointer short");
        return true;
    }
    let _src = get_u32(raw, 4);
    let dst = get_u32(raw, 8);
    let dst_x = get_u16(raw, 20) as i16;
    let dst_y = get_u16(raw, 22) as i16;
    if dst == 0 {
        // Relative warp.
        let nx = state.input.pointer_x.saturating_add(dst_x);
        let ny = state.input.pointer_y.saturating_add(dst_y);
        state.input.set_pointer(nx, ny);
    } else {
        let abs = abs_origin(state, dst);
        state.input.set_pointer(
            (abs.0 as i16).saturating_add(dst_x),
            (abs.1 as i16).saturating_add(dst_y),
        );
    }
    log::rep(c.id, seq, format_args!(
        "WarpPointer → ({},{})", state.input.pointer_x, state.input.pointer_y
    ), &[]);
    true
}

// ── Focus ───────────────────────────────────────────────────────────

// SetInputFocus (42)
//     byte 1:   revert-to (0=None, 1=PointerRoot, 2=Parent)
//     dword 4:  focus window (0=None, 1=PointerRoot, else wid)
//     dword 8:  time
fn handle_set_input_focus(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 42, 0, "SetInputFocus short");
        return true;
    }
    let revert_to = hdr.data;
    let focus = get_u32(raw, 4);
    if focus > 1 && state.resources.window(focus).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, focus, 42, 0, "SetInputFocus bad window");
        return true;
    }
    state.input.set_focus(focus, revert_to);
    log::rep(c.id, seq, format_args!("SetInputFocus {focus:#x} revert={revert_to}"), &[]);
    true
}

// GetInputFocus (43)
//
// Reply:
//     byte 1: revert-to
//     dword 8: focus
fn handle_get_input_focus(c: &mut Client, state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, state.input.focus_revert_to, 0);
    put_u32(&mut out, state.input.focus_window);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "GetInputFocus {:#x} revert={}",
        state.input.focus_window, state.input.focus_revert_to
    ), &[]);
    true
}

// QueryKeymap (44)
//
// Reply: 32 bytes of key-state bitmap (keycodes 0..=255, bit K of
// byte K/8).  We have no polling yet; reply all zeros.
fn handle_query_keymap(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    let mut out = Vec::with_capacity(32 + 32);
    put_reply_header(&mut out, seq, 0, 2);  // 2 extra words = 8 bytes... wait, 32 bytes = 8 words
    // Pad the fixed header to 32 bytes.
    while out.len() < 32 { out.push(0); }
    // 32 bytes of bitmap follow.
    for _ in 0..32 { out.push(0); }
    // Fix the extra_words count: 32 extra bytes = 8 extra words.
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("QueryKeymap (zero)"), &[]);
    true
}

// ── Keyboard / pointer mapping ─────────────────────────────────────

// GetKeyboardMapping (101)
//     byte 1: (unused)
//     dword 4: first-keycode (byte 4)
//     byte 5:  count
fn handle_get_keyboard_mapping(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 101, 0, "GetKeyboardMapping short");
        return true;
    }
    let first = raw[4];
    let count = raw[5];
    let per = keymap::KEYSYMS_PER_KEYCODE;
    let mut out = Vec::new();
    put_reply_header(&mut out, seq, per, 0);   // byte1 = keysyms-per-keycode
    while out.len() < 32 { out.push(0); }
    for k in 0..count {
        let kc = first.wrapping_add(k);
        for level in 0..per {
            let sym = keymap::lookup(kc, level as usize);
            put_u32(&mut out, sym);
        }
    }
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "GetKeyboardMapping first={first} count={count} per={per}"
    ), &[]);
    true
}

fn handle_change_keyboard_control_stub(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("ChangeKeyboardControl (stub)"), &[]);
    true
}

// GetKeyboardControl (103)
//
// Reply body (after 32-byte header):
//   byte 1: global_auto_repeat (1 = on)
//   dword 8: led_mask (all 0)
//   byte 12: key_click_percent
//   byte 13: bell_percent
//   word 14: bell_pitch
//   word 16: bell_duration
//   2 bytes pad
//   32 bytes: auto_repeats bitmap
fn handle_get_keyboard_control(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _raw: &[u8],
) -> bool {
    let mut out = Vec::new();
    put_reply_header(&mut out, seq, 1, 0);  // byte1 = global_auto_repeat = 1
    put_u32(&mut out, 0);                   // led_mask
    put_u8(&mut out, 50);                   // key_click
    put_u8(&mut out, 50);                   // bell_percent
    put_u16(&mut out, 400);                 // bell_pitch
    put_u16(&mut out, 100);                 // bell_duration
    put_u16(&mut out, 0);                   // pad
    while out.len() < 32 { out.push(0); }
    // auto_repeats: 32 bytes, all-on by default.
    for _ in 0..32 { out.push(0xFF); }
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetKeyboardControl"), &[]);
    true
}

fn handle_bell_stub(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    log::rep(c.id, seq, format_args!("Bell (stub)"), &[]);
    true
}

// ── Phase 12: miscellaneous query/control batch ────────────────────

// ChangePointerControl (105) — no-op; we don't track acceleration.
fn handle_change_pointer_control_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("ChangePointerControl (stub)"), &[]);
    true
}

// GetPointerControl (106)
// Reply body:
//   byte 1: pad
//   dword 4: reply length = 0
//   word 8:  acceleration-numerator
//   word 10: acceleration-denominator
//   word 12: threshold
//   18 bytes pad
fn handle_get_pointer_control(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u16(&mut out, 1);   // accel numerator
    put_u16(&mut out, 1);   // accel denominator
    put_u16(&mut out, 0);   // threshold
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetPointerControl → 1/1 thresh=0"), &[]);
    true
}

// SetScreenSaver (107)
fn handle_set_screen_saver_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("SetScreenSaver (stub)"), &[]);
    true
}

// GetScreenSaver (108)
// Reply body:
//   byte 1: pad
//   dword 4: reply length = 0
//   word 8:  timeout
//   word 10: interval
//   byte 12: prefer-blanking
//   byte 13: allow-exposures
//   18 bytes pad
fn handle_get_screen_saver(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u16(&mut out, 0);   // timeout (disabled)
    put_u16(&mut out, 0);   // interval
    put_u8(&mut out, 0);    // prefer_blanking = No
    put_u8(&mut out, 0);    // allow_exposures = No
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetScreenSaver → disabled"), &[]);
    true
}

// ChangeHosts (109) — we don't do access control.
fn handle_change_hosts_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("ChangeHosts (stub)"), &[]);
    true
}

// ListHosts (110)
// Reply body:
//   byte 1: mode (1 = Enabled)
//   dword 4: reply length = 0
//   word 8: nHosts = 0
//   22 bytes pad
fn handle_list_hosts(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 1, 0); // byte1 = mode (Enabled)
    put_u16(&mut out, 0);                  // num hosts
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("ListHosts → 0 hosts (access enabled)"), &[]);
    true
}

// SetAccessControl (111)
fn handle_set_access_control_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("SetAccessControl (stub)"), &[]);
    true
}

// SetCloseDownMode (112) — controls resource lifetime on disconnect.
fn handle_set_close_down_mode_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("SetCloseDownMode (stub)"), &[]);
    true
}

// KillClient (113)
fn handle_kill_client_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("KillClient (stub)"), &[]);
    true
}

// ForceScreenSaver (115)
fn handle_force_screen_saver_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("ForceScreenSaver (stub)"), &[]);
    true
}

// SetPointerMapping (116) — accept every request as success.
fn handle_set_pointer_mapping(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    _raw: &[u8],
) -> bool {
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);   // byte1 = status Success
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("SetPointerMapping → Success"), &[]);
    true
}

// GetPointerMapping (117)
// Reply:
//   byte 1: length (count of buttons)
//   count bytes of mapping entries (identity: button N → N)
fn handle_get_pointer_mapping(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    let n: u8 = 5;
    let mut out = Vec::new();
    put_reply_header(&mut out, seq, n, 0);
    while out.len() < 32 { out.push(0); }
    for i in 1..=n { out.push(i); }
    while out.len() % 4 != 0 { out.push(0); }
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetPointerMapping n={n}"), &[]);
    true
}

// SetModifierMapping (118)
fn handle_set_modifier_mapping(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    _hdr: &RequestHeader,
    _raw: &[u8],
) -> bool {
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0); // byte1 = status Success
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("SetModifierMapping → Success"), &[]);
    true
}

// GetModifierMapping (119)
//   Reply byte1: keycodes-per-modifier N
//   Body: 8 * N bytes (modifier_slot × N keycodes)
fn handle_get_modifier_mapping(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    let n = keymap::MODMAP_KEYS_PER_MOD;
    let mut out = Vec::new();
    put_reply_header(&mut out, seq, n, 0);
    while out.len() < 32 { out.push(0); }
    for row in &keymap::MOD_MAPPING {
        for k in 0..n as usize {
            out.push(row[k]);
        }
    }
    while out.len() % 4 != 0 { out.push(0); }
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("GetModifierMapping per_mod={n}"), &[]);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Phase 8 — Window-manager support
// ═════════════════════════════════════════════════════════════════════

/// Install or clear a SubstructureRedirect owner for `parent`.  X11
/// permits at most one redirect per window; if a different client
/// tried to grab SubstructureRedirect on a window that already has
/// one, that request would fail with BadAccess.  We don't enforce
/// that yet (fine for twm; only one WM usually runs), but we do
/// ensure each (parent, client) pair has at most one entry.
fn update_redirect_owner(state: &mut ServerState, parent: u32, client: u32, new_mask: u32) {
    state.redirect_owners.retain(|e| !(e.parent == parent && e.client == client));
    if new_mask & em::SUBSTRUCTURE_REDIRECT != 0 {
        state.redirect_owners.push(crate::state::RedirectOwner { parent, client });
        log::info(format_args!("redirect owner registered: parent={parent:#x} C{client}"));
    }
}

fn find_redirect_owner(state: &ServerState, parent: u32) -> Option<u32> {
    state.redirect_owners.iter().find(|e| e.parent == parent).map(|e| e.client)
}

// ReparentWindow (7)
//     dword 4:  window
//     dword 8:  parent (new)
//     word 12:  x
//     word 14:  y
fn handle_reparent_window(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 7, 0, "ReparentWindow short");
        return true;
    }
    let wid = get_u32(raw, 4);
    let new_parent = get_u32(raw, 8);
    let x = get_u16(raw, 12) as i16;
    let y = get_u16(raw, 14) as i16;
    // Lookups.
    let (old_parent, was_mapped, override_redirect) = match state.resources.window(wid) {
        Some(w) => (w.parent, w.mapped, w.override_redirect),
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 7, 0, "ReparentWindow bad window");
            return true;
        }
    };
    if state.resources.window(new_parent).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, new_parent, 7, 0, "ReparentWindow bad new parent");
        return true;
    }
    if old_parent == new_parent {
        log::rep(c.id, seq, format_args!("ReparentWindow {wid:#x} → same parent"), &[]);
        return true;
    }
    // Per X11: if the window is mapped, the server implicitly
    // unmaps it first.  We skip sending UnmapNotify here — the
    // matching MapWindow the WM issues after reparenting is what
    // puts things back.
    if was_mapped {
        if let Some(win) = state.resources.window_mut(wid) {
            win.mapped = false;
        }
    }
    // Detach from old parent's child list.
    if old_parent != 0 {
        if let Some(p) = state.resources.window_mut(old_parent) {
            p.children.retain(|id| *id != wid);
        }
    }
    // Update the window's parent pointer and position.
    if let Some(win) = state.resources.window_mut(wid) {
        win.parent = new_parent;
        win.x = x;
        win.y = y;
    }
    // Attach to new parent's child list.
    if let Some(p) = state.resources.window_mut(new_parent) {
        p.children.push(wid);
    }

    // Emit ReparentNotify to:
    //   - the window itself (STRUCTURE_NOTIFY listeners)
    //   - old parent (SUBSTRUCTURE_NOTIFY listeners)
    //   - new parent (SUBSTRUCTURE_NOTIFY listeners)
    let ev_self = event::reparent_notify(wid, wid, new_parent, x, y, override_redirect);
    deliver_to_structure(c, state, wid, em::STRUCTURE_NOTIFY, ev_self);
    if old_parent != 0 {
        let ev_old = event::reparent_notify(old_parent, wid, new_parent, x, y, override_redirect);
        deliver_to_substructure(c, state, old_parent, em::SUBSTRUCTURE_NOTIFY, ev_old);
    }
    let ev_new = event::reparent_notify(new_parent, wid, new_parent, x, y, override_redirect);
    deliver_to_substructure(c, state, new_parent, em::SUBSTRUCTURE_NOTIFY, ev_new);

    log::rep(c.id, seq, format_args!(
        "ReparentWindow {wid:#x}: {old_parent:#x} → {new_parent:#x} @ ({x},{y})"
    ), &[]);
    true
}

// CirculateWindow (13)
//     byte 1:  direction (0=RaiseLowest, 1=LowerHighest)
//     dword 4: window
fn handle_circulate_window(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 13, 0, "CirculateWindow short");
        return true;
    }
    let direction = hdr.data;  // 0=RaiseLowest, 1=LowerHighest
    let wid = get_u32(raw, 4);
    let parent = match state.resources.window(wid) {
        Some(w) => w.parent,
        None => {
            send_error(c, errcode::BAD_WINDOW, seq, wid, 13, 0, "CirculateWindow bad window");
            return true;
        }
    };
    // Pre-dispatch redirect: CirculateRequest to the parent's owner.
    if let Some(owner) = find_redirect_owner(state, parent) {
        if owner != c.id {
            let place = direction;  // same encoding
            let ev = event::circulate_request(parent, wid, place);
            queue_event_to_client(c, state, owner, ev);
            log::rep(c.id, seq, format_args!(
                "CirculateWindow {wid:#x} → CirculateRequest to C{owner}"
            ), &[]);
            return true;
        }
    }
    // Rotate the parent's children list.  Our renderer ignores
    // stacking today, so this is bookkeeping only.
    if let Some(p) = state.resources.window_mut(parent) {
        if p.children.len() >= 2 {
            match direction {
                0 => {
                    // RaiseLowest: move the last child to the front.
                    let last = p.children.pop().unwrap();
                    p.children.insert(0, last);
                }
                1 => {
                    // LowerHighest: move the first child to the back.
                    let first = p.children.remove(0);
                    p.children.push(first);
                }
                _ => {}
            }
        }
    }
    // Emit CirculateNotify.
    let ev_self = event::circulate_notify(wid, wid, direction);
    deliver_to_structure(c, state, wid, em::STRUCTURE_NOTIFY, ev_self);
    let ev_parent = event::circulate_notify(parent, wid, direction);
    deliver_to_substructure(c, state, parent, em::SUBSTRUCTURE_NOTIFY, ev_parent);
    log::rep(c.id, seq, format_args!("CirculateWindow {wid:#x} dir={direction}"), &[]);
    true
}

// SetSelectionOwner (22)
//     dword 4:  window (owner, 0=None)
//     dword 8:  selection (atom)
//     dword 12: time
fn handle_set_selection_owner(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 22, 0, "SetSelectionOwner short");
        return true;
    }
    let owner = get_u32(raw, 4);
    let selection = get_u32(raw, 8);
    let time = get_u32(raw, 12);
    if owner != 0 && state.resources.window(owner).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, owner, 22, 0, "SetSelectionOwner bad window");
        return true;
    }
    // Remove any previous entry for this selection.
    state.selection_owners.retain(|e| e.selection != selection);
    if owner != 0 {
        state.selection_owners.push(crate::state::SelectionOwner {
            selection, owner, client: c.id, time,
        });
    }
    log::rep(c.id, seq, format_args!(
        "SetSelectionOwner selection={selection:#x} owner={owner:#x}"
    ), &[]);
    true
}

// GetSelectionOwner (23)
fn handle_get_selection_owner(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 23, 0, "GetSelectionOwner short");
        return true;
    }
    let selection = get_u32(raw, 4);
    let owner = state.selection_owners.iter()
        .find(|e| e.selection == selection)
        .map(|e| e.owner)
        .unwrap_or(0);
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u32(&mut out, owner);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "GetSelectionOwner selection={selection:#x} → {owner:#x}"
    ), &[]);
    true
}

// ConvertSelection (24) — deliver a SelectionNotify with property=0.
// Real WMs that own the selection would see SelectionRequest instead;
// we deliver a cancel (property=None) so clients stop waiting.
fn handle_convert_selection(
    c: &mut Client,
    _state: &mut ServerState,
    seq: u16,
    raw: &[u8],
) -> bool {
    if raw.len() < 24 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 24, 0, "ConvertSelection short");
        return true;
    }
    // For now: log the attempt and return.  Event delivery to the
    // requesting client happens via the SendEvent path once a WM
    // owns the selection; without an owner there is nothing to do.
    log::rep(c.id, seq, format_args!("ConvertSelection (stub)"), &[]);
    true
}

// SendEvent (25)
//     byte 1:  propagate flag
//     dword 4: destination (window id, 0=PointerWindow, 1=InputFocus)
//     dword 8: event_mask
//     bytes 12..44: 32-byte event block
//
// Per spec, the event must have the high bit (0x80) set in its code
// on delivery to indicate it was synthesized.
fn handle_send_event(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    if raw.len() < 44 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 25, 0, "SendEvent short");
        return true;
    }
    let _propagate = hdr.data != 0;
    let dest = get_u32(raw, 4);
    let event_mask = get_u32(raw, 8);
    let mut ev = [0u8; 32];
    ev.copy_from_slice(&raw[12..44]);
    // Set the sent bit.
    ev[0] |= 0x80;

    let target_window = match dest {
        0 => {
            // PointerWindow — we don't track which window is under
            // the pointer; fall back to root.
            setup::ROOT_WINDOW_ID
        }
        1 => {
            // InputFocus.  Use the current focus window.
            let f = state.input.focus_window;
            if f == 0 { setup::ROOT_WINDOW_ID } else { f }
        }
        w => w,
    };
    if state.resources.window(target_window).is_none() {
        send_error(c, errcode::BAD_WINDOW, seq, target_window, 25, 0, "SendEvent bad dest");
        return true;
    }
    // Collect listeners matching the requested mask — or, per spec,
    // if event_mask is 0, deliver to *every* client that has selected
    // on the window regardless of mask.
    let listeners: Vec<(u32, u32)> = state.resources.window(target_window)
        .map(|w| w.listeners.iter().map(|l| (l.client, l.mask)).collect())
        .unwrap_or_default();
    let mut delivered = 0;
    for (client_id, mask) in listeners {
        if event_mask == 0 || (mask & event_mask) != 0 {
            queue_event_to_client(c, state, client_id, ev);
            delivered += 1;
        }
    }
    log::rep(c.id, seq, format_args!(
        "SendEvent → wid={target_window:#x} mask={event_mask:#x} delivered={delivered}"
    ), &[]);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Phase 9 — Extension negotiation + BIG-REQUESTS
// ═════════════════════════════════════════════════════════════════════
//
// QueryExtension is the single most load-bearing call in early client
// startup.  Xlib and xcb issue one per extension the client library
// wants to use (RENDER, XKB, XInput, MIT-SHM, XFIXES, DAMAGE, SHAPE,
// XKEYBOARD, XINERAMA, SYNC, ...) — the replies gate whether the
// client falls back to the core protocol or hangs.  For Phase 9 we
// advertise exactly one extension (`BIG-REQUESTS`) and say "not
// present" for everything else, which lets most X clients progress.

/// Known extensions keyed by the exact name clients ask for.
/// Returns (major_opcode, first_event, first_error).
fn extension_lookup(name: &[u8]) -> Option<(u8, u8, u8)> {
    match name {
        b"BIG-REQUESTS" => Some((128, 0, 0)),
        b"RENDER"       => Some((129, 0, 0)),
        b"XFIXES"       => Some((130, 0, 0)),
        _ => None,
    }
}

/// Names advertised by ListExtensions, in the order they appear on
/// the wire.
const ADVERTISED_EXTENSIONS: &[&[u8]] = &[
    b"BIG-REQUESTS",
    b"RENDER",
    b"XFIXES",
];

// QueryExtension (98)
//     word 4:  name length n
//     word 6:  unused
//     bytes 8..8+n: STRING8 name + pad4
//
// Reply (32 bytes):
//     byte 8:  present
//     byte 9:  major_opcode
//     byte 10: first_event
//     byte 11: first_error
fn handle_query_extension(c: &mut Client, _state: &mut ServerState, seq: u16, raw: &[u8]) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 98, 0, "QueryExtension short");
        return true;
    }
    let nlen = get_u16(raw, 4) as usize;
    if raw.len() < 8 + nlen {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 98, 0, "QueryExtension name overflow");
        return true;
    }
    let name = &raw[8..8 + nlen];
    let (present, major, first_ev, first_err) = match extension_lookup(name) {
        Some((m, e, r)) => (1u8, m, e, r),
        None            => (0u8, 0, 0, 0),
    };
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u8(&mut out, present);
    put_u8(&mut out, major);
    put_u8(&mut out, first_ev);
    put_u8(&mut out, first_err);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "QueryExtension {:?} → present={present}",
        core::str::from_utf8(name).unwrap_or("<bin>")
    ), &[]);
    true
}

// ListExtensions (99)
//     Reply:
//       byte 1: number of names
//       body: LISTofSTR (each: 1-byte length + bytes, no pad between)
//       4-byte round-up at the end
fn handle_list_extensions(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    let mut body = Vec::new();
    for name in ADVERTISED_EXTENSIONS {
        body.push(name.len() as u8);
        body.extend_from_slice(name);
    }
    while body.len() % 4 != 0 { body.push(0); }
    let mut out = Vec::with_capacity(32 + body.len());
    put_reply_header(&mut out, seq, ADVERTISED_EXTENSIONS.len() as u8, 0);
    while out.len() < 32 { out.push(0); }
    out.extend_from_slice(&body);
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "ListExtensions → {} name(s)", ADVERTISED_EXTENSIONS.len()
    ), &[]);
    true
}

// NoOperation (127) — accept with no reply.  Doubles as a "setup
// finished" sync marker for --inject: the server arms the injection
// trigger here so tests can send NoOperation after their last
// setup request and rely on injection firing immediately.
fn handle_no_operation(c: &mut Client, state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    state.inject_armed = true;
    log::rep(c.id, seq, format_args!("NoOperation (inject armed)"), &[]);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Phase 9 — Input event routing
// ═════════════════════════════════════════════════════════════════════
//
// Device readers produce `InputEvent`s; this function converts them
// to wire 32-byte events and stages them on `state.pending_events`
// for `server::poll_once` to route to the right client.
//
// Routing rules (simplified for Phase 9):
//
//   * Mouse motion:  find the window under the pointer (walk the
//     tree from root, looking for the deepest mapped window that
//     contains the screen-absolute pointer) and deliver
//     MotionNotify to every listener on that window or any ancestor
//     that selected POINTER_MOTION.  Propagation stops at windows
//     in the target's `do_not_propagate` mask.
//
//   * Mouse button: same window lookup + BUTTON_PRESS / BUTTON_RELEASE.
//
//   * Keyboard:     deliver to the focus window (StructureNotify
//     tree) with KEY_PRESS / KEY_RELEASE.
//
// Grab-aware routing (Phase 7's GrabPointer / GrabKeyboard) is not
// yet honored — those still return GrabSuccess without redirecting.
// Hooking the grab table in is a minor follow-up inside this same
// function; the data already lives on `state`.

pub fn route_input_event(state: &mut ServerState, ev: crate::device::InputEvent) {
    use crate::device::InputEvent as IE;
    match ev {
        IE::MouseMotion { dx, dy } => {
            let nx = state.input.pointer_x.saturating_add(dx);
            let ny = state.input.pointer_y.saturating_add(dy);
            state.input.set_pointer(nx, ny);
            let (x, y) = (state.input.pointer_x, state.input.pointer_y);
            let target = window_under_pointer(state, x as i32, y as i32);
            route_motion(state, target, x, y);
        }
        IE::MouseButton { button, pressed } => {
            if pressed {
                state.input.button_down(button);
            } else {
                state.input.button_up(button);
            }
            let (x, y) = (state.input.pointer_x, state.input.pointer_y);
            let target = window_under_pointer(state, x as i32, y as i32);
            route_button(state, target, button, pressed, x, y);
        }
        IE::Key { keycode, pressed } => {
            // Update shift/control/etc. state from the keysym if the
            // keycode is a modifier.
            update_modifier_state(state, keycode, pressed);
            let focus = state.input.focus_window;
            let focus = if focus == 0 || state.resources.window(focus).is_none() {
                setup::ROOT_WINDOW_ID
            } else { focus };
            route_key(state, focus, keycode, pressed);
        }
    }
}

fn window_under_pointer(state: &ServerState, px: i32, py: i32) -> u32 {
    // Walk the tree depth-first, preferring later-mapped children
    // (which are drawn on top in our simple renderer).  Start from
    // root and descend into whichever child contains the point;
    // stop when there is no matching child.
    let mut cur = setup::ROOT_WINDOW_ID;
    loop {
        let Some(w) = state.resources.window(cur) else { return cur; };
        // Walk children front-to-back (latest on top in our list
        // ordering from CirculateWindow / CreateWindow.push).
        let mut hit: Option<u32> = None;
        for child_id in w.children.iter().rev() {
            let Some(child) = state.resources.window(*child_id) else { continue; };
            if !child.mapped { continue; }
            let abs = abs_origin(state, *child_id);
            if px >= abs.0 && px < abs.0 + child.width as i32
                && py >= abs.1 && py < abs.1 + child.height as i32
            {
                hit = Some(*child_id);
                break;
            }
        }
        match hit {
            Some(child) => cur = child,
            None => return cur,
        }
    }
}

fn route_motion(state: &mut ServerState, target: u32, x: i16, y: i16) {
    let abs = abs_origin(state, target);
    let ex = (x as i32 - abs.0) as i16;
    let ey = (y as i32 - abs.1) as i16;
    let ev = event::motion_notify(
        0, 0, setup::ROOT_WINDOW_ID, target, 0,
        (x, y), (ex, ey), state.input.mask,
    );
    // Pick up any listeners from target on up the parent chain
    // that selected POINTER_MOTION and deliver to each.
    deliver_up_tree(
        state, target, em::POINTER_MOTION, ev,
        |w| !is_do_not_propagate(w, em::POINTER_MOTION),
    );
}

fn route_button(
    state: &mut ServerState, target: u32, button: u8, pressed: bool, x: i16, y: i16,
) {
    let abs = abs_origin(state, target);
    let ex = (x as i32 - abs.0) as i16;
    let ey = (y as i32 - abs.1) as i16;
    let ev = if pressed {
        event::button_press(
            button, 0, setup::ROOT_WINDOW_ID, target, 0,
            (x, y), (ex, ey), state.input.mask,
        )
    } else {
        event::button_release(
            button, 0, setup::ROOT_WINDOW_ID, target, 0,
            (x, y), (ex, ey), state.input.mask,
        )
    };
    let required = if pressed { em::BUTTON_PRESS } else { em::BUTTON_RELEASE };
    deliver_up_tree(
        state, target, required, ev,
        |w| !is_do_not_propagate(w, required),
    );
}

fn route_key(state: &mut ServerState, target: u32, keycode: u8, pressed: bool) {
    let abs = abs_origin(state, target);
    let (px, py) = (state.input.pointer_x, state.input.pointer_y);
    let ex = (px as i32 - abs.0) as i16;
    let ey = (py as i32 - abs.1) as i16;
    let ev = if pressed {
        event::key_press(
            keycode, 0, setup::ROOT_WINDOW_ID, target, 0,
            (px, py), (ex, ey), state.input.mask,
        )
    } else {
        event::key_release(
            keycode, 0, setup::ROOT_WINDOW_ID, target, 0,
            (px, py), (ex, ey), state.input.mask,
        )
    };
    let required = if pressed { em::KEY_PRESS } else { em::KEY_RELEASE };
    deliver_up_tree(
        state, target, required, ev,
        |w| !is_do_not_propagate(w, required),
    );
}

/// Walk from `start` up the parent chain, delivering `ev` to any
/// listener whose mask matches `required`.  `should_propagate`
/// decides whether to continue up (used to respect the
/// `do_not_propagate` mask).
fn deliver_up_tree<F>(
    state: &mut ServerState,
    start: u32,
    required: u32,
    ev: [u8; 32],
    should_propagate: F,
)
where F: Fn(&Window) -> bool
{
    let mut cur = start;
    loop {
        let (listeners, parent, propagate) = {
            let Some(w) = state.resources.window(cur) else { return; };
            let listeners: Vec<(u32, u32)> =
                w.listeners.iter().map(|l| (l.client, l.mask)).collect();
            (listeners, w.parent, should_propagate(w))
        };
        for (client_id, mask) in listeners {
            if mask & required != 0 {
                state.pending_events.push(crate::state::PendingEvent {
                    target_client: client_id,
                    ev,
                });
            }
        }
        if !propagate || parent == 0 || parent == cur {
            return;
        }
        cur = parent;
    }
}

fn is_do_not_propagate(w: &Window, required: u32) -> bool {
    w.do_not_propagate & required != 0
}

fn update_modifier_state(state: &mut ServerState, keycode: u8, pressed: bool) {
    // Consult the mod map to find which modifier bit (if any) this
    // keycode carries.  The table is small — linear scan.
    for (row_idx, row) in keymap::MOD_MAPPING.iter().enumerate() {
        for &kc in row {
            if kc == keycode {
                let bit: u16 = 1 << row_idx;
                state.input.set_modifier(bit, pressed);
                return;
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════
// BIG-REQUESTS extension (major opcode 128)
// ═════════════════════════════════════════════════════════════════════
//
// BIG-REQUESTS has exactly one request: BigReqEnable (minor 0).  The
// reply body is a single CARD32 "maximum-request-length" in 4-byte
// units.  After a successful BigReqEnable, clients are permitted to
// send requests with a 32-bit length word inserted after the usual
// 16-bit length field (when the 16-bit length is 0).  We accept that
// wire encoding in `RequestHeader::parse` already (see `length=0`
// case below), so all BigReqEnable does is tell the client "yes go
// ahead" and report our limit.

fn handle_big_req_enable(c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8]) -> bool {
    // Our limit is conservative: 1 MB worth of 4-byte words.
    const MAX_LEN_WORDS: u32 = 262_144;
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u32(&mut out, MAX_LEN_WORDS);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("BigReqEnable → max_len={MAX_LEN_WORDS} words"), &[]);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Phase 10 — RENDER extension (major opcode 129)
// ═════════════════════════════════════════════════════════════════════

fn handle_render_request(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    match hdr.data {
        0  => handle_render_query_version(c, state, seq, raw),
        1  => handle_render_query_pict_formats(c, state, seq, raw),
        4  => handle_render_create_picture(c, state, seq, raw),
        5  => handle_render_change_picture_stub(c, state, seq, raw),
        6  => handle_render_set_picture_clip_rectangles(c, state, seq, raw),
        7  => handle_render_free_picture(c, state, seq, raw),
        8  => handle_render_composite(c, state, seq, hdr, raw),
        17 => handle_render_create_glyph_set(c, state, seq, raw),
        19 => handle_render_free_glyph_set(c, state, seq, raw),
        20 => handle_render_add_glyphs(c, state, seq, raw),
        22 => handle_render_free_glyphs_stub(c, state, seq, raw),
        23 => handle_render_composite_glyphs_8(c, state, seq, hdr, raw),
        26 => handle_render_fill_rectangles(c, state, seq, hdr, raw),
        27 => handle_render_create_cursor_stub(c, state, seq, raw),
        33 => handle_render_create_solid_fill(c, state, seq, raw),
        minor => {
            log::warn(format_args!("RENDER minor {minor} not implemented"));
            // Return BadImplementation so clients can see the gap.
            send_error(c, errcode::BAD_IMPLEMENTATION, seq, 0, 129, minor as u16,
                       "RENDER minor not implemented");
            true
        }
    }
}

// ── QueryVersion (0) ───────────────────────────────────────────────
// Reply:
//   byte 1: pad
//   dword 4: reply length = 0
//   dword 8:  major_version (client's request echoed, clamped)
//   dword 12: minor_version
//   16 pad bytes
fn handle_render_query_version(
    c: &mut Client, _state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 0, "RENDER QueryVersion short");
        return true;
    }
    let client_major = get_u32(raw, 4);
    let client_minor = get_u32(raw, 8);
    // Advertise version 0.11 (the version Xft expects as a minimum).
    let major: u32 = client_major.min(0);
    let minor: u32 = if client_major == 0 { client_minor.min(11) } else { 11 };
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u32(&mut out, major);
    put_u32(&mut out, minor);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "RENDER QueryVersion → {major}.{minor}"
    ), &[]);
    true
}

// ── QueryPictFormats (1) ───────────────────────────────────────────
//
// The reply body is:
//   byte 1: pad
//   dword 8:  num_formats
//   dword 12: num_screens
//   dword 16: num_depths
//   dword 20: num_visuals
//   dword 24: num_subpixel
//   4 pad
//
//   LISTofPICTFORMINFO (28 bytes each):
//     dword id
//     byte  type
//     byte  depth
//     2 pad
//     word red_shift   word red_mask
//     word green_shift word green_mask
//     word blue_shift  word blue_mask
//     word alpha_shift word alpha_mask
//     dword colormap
//
//   LISTofPICTSCREEN:
//     for each screen:
//       dword num_depths
//       dword fallback_format
//       LISTofPICTDEPTH:
//         byte depth
//         byte pad
//         word num_visuals
//         dword pad
//         LISTofPICTVISUAL:
//           dword visual_id
//           dword picture_format
//
//   LISTofCARD32 subpixels (one per screen)
fn handle_render_query_pict_formats(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    let formats = render_ext::PICT_FORMATS;
    let num_formats = formats.len() as u32;
    let num_screens = 1u32;
    let num_depths = 1u32;   // just depth 24 on our one screen
    let num_visuals = 1u32;  // the root visual
    let num_subpixel = 1u32; // one entry per screen

    let mut out = Vec::new();
    put_reply_header(&mut out, seq, 0, 0); // patch length later
    put_u32(&mut out, num_formats);
    put_u32(&mut out, num_screens);
    put_u32(&mut out, num_depths);
    put_u32(&mut out, num_visuals);
    put_u32(&mut out, num_subpixel);
    for _ in 0..4 { out.push(0); }
    debug_assert!(out.len() >= 32);
    // Pad to 32 bytes if we fell short.
    while out.len() < 32 { out.push(0); }

    // PICTFORMINFO × num_formats
    for f in formats {
        put_u32(&mut out, f.id);
        put_u8(&mut out, f.ty);
        put_u8(&mut out, f.depth);
        put_u16(&mut out, 0);        // 2 pad
        put_u16(&mut out, f.direct.red_shift);
        put_u16(&mut out, f.direct.red_mask);
        put_u16(&mut out, f.direct.green_shift);
        put_u16(&mut out, f.direct.green_mask);
        put_u16(&mut out, f.direct.blue_shift);
        put_u16(&mut out, f.direct.blue_mask);
        put_u16(&mut out, f.direct.alpha_shift);
        put_u16(&mut out, f.direct.alpha_mask);
        put_u32(&mut out, f.colormap);
    }

    // PICTSCREEN × num_screens
    //   num_depths (we have 1), fallback_format
    put_u32(&mut out, num_depths);
    put_u32(&mut out, render_ext::PICTFORMAT_X8R8G8B8);
    // PICTDEPTH × num_depths
    put_u8(&mut out, 24); // depth
    put_u8(&mut out, 0);  // pad
    put_u16(&mut out, num_visuals as u16);
    put_u32(&mut out, 0); // pad
    // PICTVISUAL × num_visuals
    put_u32(&mut out, setup::ROOT_VISUAL_ID);
    put_u32(&mut out, render_ext::PICTFORMAT_X8R8G8B8);

    // LISTofCARD32 subpixels (one per screen).  0 = Unknown.
    put_u32(&mut out, 0);

    // Pad to a 4-byte boundary and patch extra_words.
    while out.len() % 4 != 0 { out.push(0); }
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "RENDER QueryPictFormats → {num_formats} formats"
    ), &[]);
    true
}

// ── CreatePicture (4) ──────────────────────────────────────────────
//     dword 4:  pid
//     dword 8:  drawable
//     dword 12: format
//     dword 16: value_mask
//     bytes 20+: value list
fn handle_render_create_picture(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 20 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 4, "RENDER CreatePicture short");
        return true;
    }
    let pid      = get_u32(raw, 4);
    let drawable = get_u32(raw, 8);
    let format   = get_u32(raw, 12);
    let value_mask = get_u32(raw, 16);
    let _values  = &raw[20..];
    if !belongs_to_client(c.id, pid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, pid, 129, 4,
                   "RENDER CreatePicture id not in client range");
        return true;
    }
    if state.resources.get(pid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, pid, 129, 4,
                   "RENDER CreatePicture id in use");
        return true;
    }
    if !render_ext::pict_format_exists(format) {
        send_error(c, errcode::BAD_MATCH, seq, format, 129, 4,
                   "RENDER CreatePicture unknown format");
        return true;
    }
    if !is_drawable(state, drawable) {
        send_error(c, errcode::BAD_DRAWABLE, seq, drawable, 129, 4,
                   "RENDER CreatePicture bad drawable");
        return true;
    }
    state.resources.insert(pid, c.id, Resource::Picture(Picture::new_drawable(drawable, format)));
    log::rep(c.id, seq, format_args!(
        "RENDER CreatePicture pid={pid:#x} drw={drawable:#x} fmt={format:#x} mask={value_mask:#x}"
    ), &[]);
    true
}

// ── ChangePicture (5) ──────────────────────────────────────────────
fn handle_render_change_picture_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("RENDER ChangePicture (stub)"), &[]);
    true
}

// ── SetPictureClipRectangles (6) ───────────────────────────────────
fn handle_render_set_picture_clip_rectangles(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 6, "RENDER SetPictureClipRects short");
        return true;
    }
    let pid = get_u32(raw, 4);
    let _clip_x = get_u16(raw, 8) as i16;
    let _clip_y = get_u16(raw, 10) as i16;
    let rect_bytes = &raw[12..];
    let n = rect_bytes.len() / 8;
    let mut rects = Vec::with_capacity(n);
    for i in 0..n {
        let x = i16::from_le_bytes([rect_bytes[i*8],   rect_bytes[i*8+1]]);
        let y = i16::from_le_bytes([rect_bytes[i*8+2], rect_bytes[i*8+3]]);
        let w = u16::from_le_bytes([rect_bytes[i*8+4], rect_bytes[i*8+5]]);
        let h = u16::from_le_bytes([rect_bytes[i*8+6], rect_bytes[i*8+7]]);
        rects.push((x, y, w, h));
    }
    match state.resources.picture_mut(pid) {
        Some(p) => { p.clip_rects = rects; }
        None => {
            send_error(c, errcode::BAD_VALUE, seq, pid, 129, 6,
                       "RENDER SetPictureClipRects bad picture");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!(
        "RENDER SetPictureClipRects pid={pid:#x} n={n}"
    ), &[]);
    true
}

// ── FreePicture (7) ────────────────────────────────────────────────
fn handle_render_free_picture(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 7, "RENDER FreePicture short");
        return true;
    }
    let pid = get_u32(raw, 4);
    if state.resources.picture(pid).is_none() {
        send_error(c, errcode::BAD_VALUE, seq, pid, 129, 7, "RENDER FreePicture bad pid");
        return true;
    }
    state.resources.remove(pid);
    log::rep(c.id, seq, format_args!("RENDER FreePicture {pid:#x}"), &[]);
    true
}

// ── CreateCursor (27) — stub ────────────────────────────────────
//     dword 4: cid (cursor id)
//     dword 8: source_picture
//     word 12: x   word 14: y
// We accept the cursor id into the resource map as a dummy
// colormap variant (same trick as CreateGlyphCursor in Phase 6).
// Real cursors are never drawn; we don't have a pointer sprite.
fn handle_render_create_cursor_stub(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 27, "RENDER CreateCursor short");
        return true;
    }
    let cid = get_u32(raw, 4);
    if !belongs_to_client(c.id, cid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, cid, 129, 27,
                   "RENDER CreateCursor id not in client range");
        return true;
    }
    if state.resources.get(cid).is_none() {
        state.resources.insert(cid, c.id, Resource::Colormap { visual: 0 });
    }
    log::rep(c.id, seq, format_args!("RENDER CreateCursor {cid:#x} (stub)"), &[]);
    true
}

// ── CreateSolidFill (33) ───────────────────────────────────────────
//     dword 4: pid
//     word 8, 10, 12, 14: red, green, blue, alpha (all u16)
fn handle_render_create_solid_fill(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 33, "RENDER CreateSolidFill short");
        return true;
    }
    let pid = get_u32(raw, 4);
    let r16 = get_u16(raw, 8);
    let g16 = get_u16(raw, 10);
    let b16 = get_u16(raw, 12);
    let a16 = get_u16(raw, 14);
    if !belongs_to_client(c.id, pid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, pid, 129, 33,
                   "RENDER CreateSolidFill id not in client range");
        return true;
    }
    if state.resources.get(pid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, pid, 129, 33,
                   "RENDER CreateSolidFill id in use");
        return true;
    }
    let argb = ((a16 >> 8) as u32) << 24
             | ((r16 >> 8) as u32) << 16
             | ((g16 >> 8) as u32) << 8
             |  (b16 >> 8) as u32;
    state.resources.insert(pid, c.id, Resource::Picture(Picture::new_solid(argb)));
    log::rep(c.id, seq, format_args!(
        "RENDER CreateSolidFill pid={pid:#x} argb={argb:#010x}"
    ), &[]);
    true
}

// ── CreateGlyphSet (17) ────────────────────────────────────────────
//     dword 4: gsid
//     dword 8: format
fn handle_render_create_glyph_set(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 17, "RENDER CreateGlyphSet short");
        return true;
    }
    let gsid = get_u32(raw, 4);
    let format = get_u32(raw, 8);
    if !belongs_to_client(c.id, gsid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, gsid, 129, 17,
                   "RENDER CreateGlyphSet id not in client range");
        return true;
    }
    if state.resources.get(gsid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, gsid, 129, 17,
                   "RENDER CreateGlyphSet id in use");
        return true;
    }
    if !render_ext::pict_format_exists(format) {
        send_error(c, errcode::BAD_MATCH, seq, format, 129, 17,
                   "RENDER CreateGlyphSet unknown format");
        return true;
    }
    state.resources.insert(gsid, c.id, Resource::GlyphSet(GlyphSet::new(format)));
    log::rep(c.id, seq, format_args!(
        "RENDER CreateGlyphSet gsid={gsid:#x} fmt={format:#x}"
    ), &[]);
    true
}

// ── FreeGlyphSet (19) ──────────────────────────────────────────────
fn handle_render_free_glyph_set(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 19, "RENDER FreeGlyphSet short");
        return true;
    }
    let gsid = get_u32(raw, 4);
    if state.resources.glyph_set(gsid).is_none() {
        send_error(c, errcode::BAD_VALUE, seq, gsid, 129, 19, "RENDER FreeGlyphSet bad gsid");
        return true;
    }
    state.resources.remove(gsid);
    log::rep(c.id, seq, format_args!("RENDER FreeGlyphSet {gsid:#x}"), &[]);
    true
}

// ── AddGlyphs (20) ─────────────────────────────────────────────────
//     dword 4:  gsid
//     dword 8:  nglyphs
//     LISTofCARD32 glyph_ids
//     LISTofGLYPHINFO (12 bytes each):
//       word width
//       word height
//       word x
//       word y
//       word x_off
//       word y_off
//     LISTofBYTE image_data (row-major, padded to 4 bytes per glyph)
fn handle_render_add_glyphs(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 20, "RENDER AddGlyphs short");
        return true;
    }
    let gsid = get_u32(raw, 4);
    let nglyphs = get_u32(raw, 8) as usize;
    let gs_format = match state.resources.glyph_set(gsid) {
        Some(g) => g.format,
        None => {
            send_error(c, errcode::BAD_VALUE, seq, gsid, 129, 20, "RENDER AddGlyphs bad gsid");
            return true;
        }
    };
    let bytes_per_pixel = match gs_format {
        render_ext::PICTFORMAT_A8 => 1,
        render_ext::PICTFORMAT_A1 => 0, // bit-packed
        render_ext::PICTFORMAT_A8R8G8B8 => 4,
        _ => {
            send_error(c, errcode::BAD_MATCH, seq, gs_format, 129, 20,
                       "RENDER AddGlyphs unsupported glyph format");
            return true;
        }
    };
    // Parse the glyph_ids list, then GLYPHINFOs, then image data.
    let ids_end = 12 + nglyphs * 4;
    let infos_end = ids_end + nglyphs * 12;
    if raw.len() < infos_end {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 20, "RENDER AddGlyphs body short");
        return true;
    }
    let mut ids = Vec::with_capacity(nglyphs);
    for i in 0..nglyphs {
        ids.push(get_u32(raw, 12 + i*4));
    }
    // GLYPHINFOs
    let mut infos: Vec<(u16, u16, i16, i16, i16, i16)> = Vec::with_capacity(nglyphs);
    for i in 0..nglyphs {
        let off = ids_end + i * 12;
        let w  = get_u16(raw, off);
        let h  = get_u16(raw, off + 2);
        let gx = get_u16(raw, off + 4) as i16;
        let gy = get_u16(raw, off + 6) as i16;
        let xo = get_u16(raw, off + 8) as i16;
        let yo = get_u16(raw, off + 10) as i16;
        infos.push((w, h, gx, gy, xo, yo));
    }
    let mut cursor = infos_end;
    // Extract glyphs outside the mutable borrow, then insert.
    let mut new_glyphs: Vec<(u32, Glyph)> = Vec::with_capacity(nglyphs);
    for (i, &(w, h, gx, gy, xo, yo)) in infos.iter().enumerate() {
        if bytes_per_pixel == 0 {
            // A1: 1 bit per pixel, rows padded to 4 bytes.
            let stride = ((w as usize + 31) / 32) * 4;
            let bytes = stride * h as usize;
            if raw.len() < cursor + bytes {
                send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 20, "RENDER AddGlyphs data short");
                return true;
            }
            let mut data = vec![0u8; (w as usize) * (h as usize)];
            for row in 0..h as usize {
                for col in 0..w as usize {
                    let byte = raw[cursor + row * stride + col / 8];
                    let bit = (byte >> (col & 7)) & 1;
                    data[row * w as usize + col] = if bit != 0 { 0xFF } else { 0 };
                }
            }
            cursor += bytes;
            new_glyphs.push((ids[i], Glyph { width: w, height: h, x: gx, y: gy, x_off: xo, y_off: yo, data }));
        } else {
            // Each row is padded to 4 bytes.
            let row_bytes_unpadded = w as usize * bytes_per_pixel;
            let row_bytes = (row_bytes_unpadded + 3) & !3;
            let total = row_bytes * h as usize;
            if raw.len() < cursor + total {
                send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 20, "RENDER AddGlyphs data short");
                return true;
            }
            let mut data = Vec::with_capacity(row_bytes_unpadded * h as usize);
            for row in 0..h as usize {
                let row_start = cursor + row * row_bytes;
                data.extend_from_slice(&raw[row_start..row_start + row_bytes_unpadded]);
            }
            cursor += total;
            new_glyphs.push((ids[i], Glyph { width: w, height: h, x: gx, y: gy, x_off: xo, y_off: yo, data }));
        }
    }
    // Install glyphs.
    let gs = state.resources.glyph_set_mut(gsid).unwrap();
    for (gid, glyph) in new_glyphs {
        gs.glyphs.insert(gid, glyph);
    }
    log::rep(c.id, seq, format_args!(
        "RENDER AddGlyphs gsid={gsid:#x} n={nglyphs}"
    ), &[]);
    true
}

fn handle_render_free_glyphs_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("RENDER FreeGlyphs (stub)"), &[]);
    true
}

// ── Composite (8) ──────────────────────────────────────────────────
// Extension-request wire layout: byte 1 is the MINOR OPCODE, not a
// data byte; the operator lives in the first body byte.
//     byte 4:   op (Porter-Duff operator)
//     bytes 5..8: padding
//     dword 8:  src
//     dword 12: mask
//     dword 16: dst
//     word 20:  src_x   word 22: src_y
//     word 24:  mask_x  word 26: mask_y
//     word 28:  dst_x   word 30: dst_y
//     word 32:  width   word 34: height
fn handle_render_composite(
    c: &mut Client, state: &mut ServerState, seq: u16, _hdr: &RequestHeader, raw: &[u8],
) -> bool {
    if raw.len() < 36 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 8, "RENDER Composite short");
        return true;
    }
    let op      = raw[4];
    let src_pid = get_u32(raw, 8);
    let _mask   = get_u32(raw, 12);
    let dst_pid = get_u32(raw, 16);
    let src_x   = get_u16(raw, 20) as i16;
    let src_y   = get_u16(raw, 22) as i16;
    let _mx     = get_u16(raw, 24) as i16;
    let _my     = get_u16(raw, 26) as i16;
    let dst_x   = get_u16(raw, 28) as i16;
    let dst_y   = get_u16(raw, 30) as i16;
    let w       = get_u16(raw, 32);
    let h       = get_u16(raw, 34);
    // Read source and destination pictures up front.
    let (src_drw, src_solid) = match state.resources.picture(src_pid) {
        Some(p) => (p.drawable, p.solid),
        None => {
            send_error(c, errcode::BAD_VALUE, seq, src_pid, 129, 8, "RENDER Composite bad src");
            return true;
        }
    };
    let dst = match fetch_render_dst(state, dst_pid) {
        Some(d) => d,
        None => {
            send_error(c, errcode::BAD_VALUE, seq, dst_pid, 129, 8, "RENDER Composite bad dst");
            return true;
        }
    };
    // Build the w×h source pixel buffer.
    let src_buf: Vec<u32> = if let Some(argb) = src_solid {
        vec![argb; w as usize * h as usize]
    } else if src_drw != 0 {
        read_rect(state, src_drw, src_x as i32, src_y as i32, w, h)
    } else {
        vec![0u32; w as usize * h as usize]
    };
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            let s = src_buf[(row * w as i32 + col) as usize];
            let lx = dst_x as i32 + col;
            let ly = dst_y as i32 + row;
            let d = render_read_pixel(state, &dst, lx, ly);
            let out = match op {
                render_ext::PICT_OP_CLEAR => 0,
                render_ext::PICT_OP_SRC   => s,
                render_ext::PICT_OP_OVER  => render_ext::over_pixel(s, d),
                _ => render_ext::over_pixel(s, d),
            };
            render_write_pixel(state, &dst, lx, ly, out);
        }
    }
    log::rep(c.id, seq, format_args!(
        "RENDER Composite op={op} {w}x{h} {src_pid:#x}→{dst_pid:#x}"
    ), &[]);
    true
}

// ── CompositeGlyphs8 (23) ──────────────────────────────────────────
//
// The hot path.  Real wire layout (extension request):
//     byte 1:   minor opcode
//     byte 4:   op (Porter-Duff)
//     bytes 5..8: padding
//     dword 8:  src
//     dword 12: dst
//     dword 16: mask_format (0 if none)
//     dword 20: gsid
//     word 24:  glyph_x
//     word 26:  glyph_y
//     bytes 28+: LISTofGLYPHELT8
//
// Each GLYPHELT8 is:
//     byte  nglyphs  (0xFF = font switch)
//     3 pad
//     word delta_x
//     word delta_y
//     bytes glyph_ids (nglyphs bytes)
//     pad to 4 bytes
fn handle_render_composite_glyphs_8(
    c: &mut Client, state: &mut ServerState, seq: u16, _hdr: &RequestHeader, raw: &[u8],
) -> bool {
    if raw.len() < 28 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 23, "RENDER CompositeGlyphs8 short");
        return true;
    }
    let op       = raw[4];
    let src_pid  = get_u32(raw, 8);
    let dst_pid  = get_u32(raw, 12);
    let _maskfmt = get_u32(raw, 16);
    let gsid     = get_u32(raw, 20);
    let mut glyph_x = get_u16(raw, 24) as i16;
    let mut glyph_y = get_u16(raw, 26) as i16;

    // Source picture must exist.  Pull its solid color up front.
    let (src_solid, _src_drw) = match state.resources.picture(src_pid) {
        Some(p) => (p.solid, p.drawable),
        None => {
            send_error(c, errcode::BAD_VALUE, seq, src_pid, 129, 23, "RENDER CompositeGlyphs8 bad src");
            return true;
        }
    };
    // Destination picture must be drawable-backed.
    let dst = match fetch_render_dst(state, dst_pid) {
        Some(d) => d,
        None => {
            send_error(c, errcode::BAD_VALUE, seq, dst_pid, 129, 23,
                       "RENDER CompositeGlyphs8 bad dst");
            return true;
        }
    };
    // The glyph set must exist.  We need to walk multiple glyphs so
    // snapshot (id → (w, h, x, y, x_off, y_off, data)) for the set.
    let gs_snapshot: Vec<(u32, u16, u16, i16, i16, i16, i16, Vec<u8>)> =
        match state.resources.glyph_set(gsid) {
            Some(gs) => gs.glyphs.iter().map(|(&id, g)| (
                id, g.width, g.height, g.x, g.y, g.x_off, g.y_off, g.data.clone(),
            )).collect(),
            None => {
                send_error(c, errcode::BAD_VALUE, seq, gsid, 129, 23, "RENDER CompositeGlyphs8 bad gsid");
                return true;
            }
        };
    let find_glyph = |gid: u32| -> Option<&(u32, u16, u16, i16, i16, i16, i16, Vec<u8>)> {
        gs_snapshot.iter().find(|g| g.0 == gid)
    };

    let src_argb = src_solid.unwrap_or(0xFFFFFFFF);

    // Walk the GLYPHELT8 list.
    let mut items = &raw[28..];
    while !items.is_empty() {
        if items.len() < 8 { break; }
        let n = items[0];
        // nbytes=255 means font switch — we ignore it (single-font).
        if n == 0xFF {
            if items.len() < 8 { break; }
            items = &items[8..];
            continue;
        }
        let dx = i16::from_le_bytes([items[4], items[5]]);
        let dy = i16::from_le_bytes([items[6], items[7]]);
        glyph_x = glyph_x.saturating_add(dx);
        glyph_y = glyph_y.saturating_add(dy);
        if items.len() < 8 + n as usize { break; }
        let ids = &items[8..8 + n as usize];
        for &gid in ids {
            let Some((_, gw, gh, gx, gy, xoff, _yoff, data)) = find_glyph(gid as u32)
            else {
                continue;
            };
            // The glyph is drawn at (glyph_x - gx, glyph_y - gy) in
            // destination-picture coordinates.
            let top_x = glyph_x - *gx;
            let top_y = glyph_y - *gy;
            for row in 0..*gh as i32 {
                for col in 0..*gw as i32 {
                    let alpha = data[row as usize * *gw as usize + col as usize];
                    if alpha == 0 { continue; }
                    let lx = top_x as i32 + col;
                    let ly = top_y as i32 + row;
                    let d = render_read_pixel(state, &dst, lx, ly);
                    let out = match op {
                        render_ext::PICT_OP_SRC => {
                            // src * mask replaces destination.
                            let sa = ((src_argb >> 24) & 0xFF) as u32;
                            let sr = (src_argb >> 16) & 0xFF;
                            let sg = (src_argb >> 8)  & 0xFF;
                            let sb =  src_argb        & 0xFF;
                            let ma = alpha as u32;
                            let ea = (sa * ma) / 255;
                            let er = (sr * ma) / 255;
                            let eg = (sg * ma) / 255;
                            let eb = (sb * ma) / 255;
                            (ea << 24) | (er << 16) | (eg << 8) | eb
                        }
                        _ => render_ext::over_pixel_masked(src_argb, d, alpha),
                    };
                    render_write_pixel(state, &dst, lx, ly, out);
                }
            }
            glyph_x = glyph_x.saturating_add(*xoff);
        }
        // Advance past this GLYPHELT8.  Total bytes = 8 + n + pad4(n).
        let elt_len = 8 + ((n as usize + 3) & !3);
        if items.len() < elt_len { break; }
        items = &items[elt_len..];
    }
    log::rep(c.id, seq, format_args!(
        "RENDER CompositeGlyphs8 op={op} dst={dst_pid:#x}"
    ), &[]);
    true
}

// ── FillRectangles (26) ────────────────────────────────────────────
// Real wire layout:
//     byte 1:  minor opcode
//     byte 4:  op
//     bytes 5..8: pad
//     dword 8: dst
//     word 12, 14, 16, 18: red, green, blue, alpha (u16)
//     bytes 20+: LISTofRECTANGLE (8 bytes each: x, y, w, h)
fn handle_render_fill_rectangles(
    c: &mut Client, state: &mut ServerState, seq: u16, _hdr: &RequestHeader, raw: &[u8],
) -> bool {
    if raw.len() < 20 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 129, 26, "RENDER FillRectangles short");
        return true;
    }
    let op   = raw[4];
    let pid  = get_u32(raw, 8);
    let r16  = get_u16(raw, 12);
    let g16  = get_u16(raw, 14);
    let b16  = get_u16(raw, 16);
    let a16  = get_u16(raw, 18);
    let dst = match fetch_render_dst(state, pid) {
        Some(d) => d,
        None => {
            send_error(c, errcode::BAD_VALUE, seq, pid, 129, 26, "RENDER FillRectangles bad dst");
            return true;
        }
    };
    let src = ((a16 >> 8) as u32) << 24
            | ((r16 >> 8) as u32) << 16
            | ((g16 >> 8) as u32) << 8
            |  (b16 >> 8) as u32;
    let rects = &raw[20..];
    let n = rects.len() / 8;
    for i in 0..n {
        let x = i16::from_le_bytes([rects[i*8],   rects[i*8+1]]);
        let y = i16::from_le_bytes([rects[i*8+2], rects[i*8+3]]);
        let w = u16::from_le_bytes([rects[i*8+4], rects[i*8+5]]);
        let h = u16::from_le_bytes([rects[i*8+6], rects[i*8+7]]);
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                let lx = x as i32 + col;
                let ly = y as i32 + row;
                let d = render_read_pixel(state, &dst, lx, ly);
                let out = match op {
                    render_ext::PICT_OP_CLEAR => 0,
                    render_ext::PICT_OP_SRC   => src,
                    render_ext::PICT_OP_OVER  => render_ext::over_pixel(src, d),
                    _ => render_ext::over_pixel(src, d),
                };
                render_write_pixel(state, &dst, lx, ly, out);
            }
        }
    }
    log::rep(c.id, seq, format_args!(
        "RENDER FillRectangles op={op} dst={pid:#x} n={n}"
    ), &[]);
    true
}

// ═════════════════════════════════════════════════════════════════════
// Phase 11 — XFIXES extension (major opcode 130)
// ═════════════════════════════════════════════════════════════════════
//
// The essentials for GTK/Cairo/Xft: region objects, picture-clip
// region installation, and minimal cursor-image / selection-input
// replies so clients that probe XFIXES during startup keep moving.
// Anything we don't implement returns `BadImplementation` so it's
// visible in logs for future diagnostics.

fn handle_xfixes_request(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    hdr: &RequestHeader,
    raw: &[u8],
) -> bool {
    match hdr.data {
        0  => handle_xfixes_query_version(c, state, seq, raw),
        2  => handle_xfixes_select_selection_input_stub(c, state, seq, raw),
        4  => handle_xfixes_get_cursor_image(c, state, seq, raw),
        5  => handle_xfixes_create_region(c, state, seq, raw),
        10 => handle_xfixes_destroy_region(c, state, seq, raw),
        11 => handle_xfixes_set_region(c, state, seq, raw),
        12 => handle_xfixes_copy_region(c, state, seq, raw),
        13 => handle_xfixes_union_region(c, state, seq, raw),
        14 => handle_xfixes_intersect_region(c, state, seq, raw),
        15 => handle_xfixes_subtract_region(c, state, seq, raw),
        16 => handle_xfixes_invert_region(c, state, seq, raw),
        17 => handle_xfixes_translate_region(c, state, seq, raw),
        18 => handle_xfixes_region_extents(c, state, seq, raw),
        19 => handle_xfixes_fetch_region(c, state, seq, raw),
        20 => handle_xfixes_set_gc_clip_region(c, state, seq, raw),
        22 => handle_xfixes_set_picture_clip_region(c, state, seq, raw),
        23 => handle_xfixes_set_cursor_name_stub(c, state, seq, raw),
        minor => {
            log::warn(format_args!("XFIXES minor {minor} not implemented"));
            send_error(c, errcode::BAD_IMPLEMENTATION, seq, 0, 130, minor as u16,
                       "XFIXES minor not implemented");
            true
        }
    }
}

// ── QueryVersion (0) ───────────────────────────────────────────────
// Reply: major_version / minor_version echoed back, clamped to
// server's own maximum (5.0 is the full current spec; we honestly
// implement ~3 but advertise 5 so modern clients don't disable
// themselves on the version check).
fn handle_xfixes_query_version(
    c: &mut Client, _state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 0, "XFIXES QueryVersion short");
        return true;
    }
    let client_major = get_u32(raw, 4);
    let client_minor = get_u32(raw, 8);
    let major: u32 = client_major.min(5);
    let minor: u32 = if client_major >= 5 { 0 } else { client_minor };
    let mut out = Vec::with_capacity(32);
    put_reply_header(&mut out, seq, 0, 0);
    put_u32(&mut out, major);
    put_u32(&mut out, minor);
    while out.len() < 32 { out.push(0); }
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("XFIXES QueryVersion → {major}.{minor}"), &[]);
    true
}

// ── SelectSelectionInput (2) ───────────────────────────────────────
// Used by clipboard-watching clients to receive SelectionNotify-
// style events when someone else takes the selection.  Registration
// is stored but we never generate the event yet — clipboard flow is
// deferred.
fn handle_xfixes_select_selection_input_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("XFIXES SelectSelectionInput (stub)"), &[]);
    true
}

// ── GetCursorImage (4) ─────────────────────────────────────────────
// Reply body:
//   word 8: root_x  word 10: root_y
//   word 12: width   word 14: height
//   word 16: xhot    word 18: yhot
//   dword 20: serial
//   8 bytes pad
//   LISTofCARD32 pixels (width*height entries)
//
// We return a fixed 8×8 checker cursor with a hotspot at (4, 4).
// xterm occasionally polls for the current cursor to snap its
// rendering; the content doesn't matter as long as the layout is
// right.
fn handle_xfixes_get_cursor_image(
    c: &mut Client, state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    const W: u16 = 8;
    const H: u16 = 8;
    let mut out = Vec::new();
    put_reply_header(&mut out, seq, 0, 0);     // patch length below
    put_u16(&mut out, state.input.pointer_x as u16);  // root_x
    put_u16(&mut out, state.input.pointer_y as u16);  // root_y
    put_u16(&mut out, W);                         // width
    put_u16(&mut out, H);                         // height
    put_u16(&mut out, 4);                         // xhot
    put_u16(&mut out, 4);                         // yhot
    put_u32(&mut out, 1);                         // cursor serial
    for _ in 0..8 { out.push(0); }                // 8 bytes pad
    debug_assert!(out.len() >= 32);
    while out.len() < 32 { out.push(0); }
    // 64 A8R8G8B8 pixels.
    for row in 0..H {
        for col in 0..W {
            let on = (row + col) % 2 == 0;
            let px: u32 = if on { 0xFFFFFFFF } else { 0x00000000 };
            put_u32(&mut out, px);
        }
    }
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!("XFIXES GetCursorImage ({W}x{H} stub)"), &[]);
    true
}

// ── Region helpers ─────────────────────────────────────────────────

fn xfixes_install_region(
    state: &mut ServerState,
    c_id: u32,
    rid: u32,
    region: Region,
) {
    state.resources.insert(rid, c_id, Resource::Region(region));
}

fn xfixes_get_region(state: &ServerState, rid: u32) -> Option<Region> {
    state.resources.region(rid).cloned()
}

fn xfixes_parse_rects(raw: &[u8], offset: usize) -> Vec<RegRect> {
    let n = (raw.len() - offset) / 8;
    let mut rects = Vec::with_capacity(n);
    for i in 0..n {
        let off = offset + i * 8;
        let x = i16::from_le_bytes([raw[off],   raw[off+1]]);
        let y = i16::from_le_bytes([raw[off+2], raw[off+3]]);
        let w = u16::from_le_bytes([raw[off+4], raw[off+5]]);
        let h = u16::from_le_bytes([raw[off+6], raw[off+7]]);
        rects.push(RegRect::new(x, y, w, h));
    }
    rects
}

// ── CreateRegion (5) ───────────────────────────────────────────────
//     dword 4: rid
//     bytes 8+: LISTofRECTANGLE
fn handle_xfixes_create_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 5, "XFIXES CreateRegion short");
        return true;
    }
    let rid = get_u32(raw, 4);
    if !belongs_to_client(c.id, rid) {
        send_error(c, errcode::BAD_IDCHOICE, seq, rid, 130, 5,
                   "XFIXES CreateRegion id not in client range");
        return true;
    }
    if state.resources.get(rid).is_some() {
        send_error(c, errcode::BAD_IDCHOICE, seq, rid, 130, 5,
                   "XFIXES CreateRegion id in use");
        return true;
    }
    let rects = xfixes_parse_rects(raw, 8);
    let region = Region::from_rects(rects);
    let n = region.rects.len();
    xfixes_install_region(state, c.id, rid, region);
    log::rep(c.id, seq, format_args!("XFIXES CreateRegion rid={rid:#x} n={n}"), &[]);
    true
}

// ── DestroyRegion (10) ─────────────────────────────────────────────
fn handle_xfixes_destroy_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 10, "XFIXES DestroyRegion short");
        return true;
    }
    let rid = get_u32(raw, 4);
    if state.resources.region(rid).is_none() {
        send_error(c, errcode::BAD_VALUE, seq, rid, 130, 10, "XFIXES DestroyRegion bad rid");
        return true;
    }
    state.resources.remove(rid);
    log::rep(c.id, seq, format_args!("XFIXES DestroyRegion {rid:#x}"), &[]);
    true
}

// ── SetRegion (11) ─────────────────────────────────────────────────
//     dword 4: rid
//     bytes 8+: rectangles (replace existing contents)
fn handle_xfixes_set_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 11, "XFIXES SetRegion short");
        return true;
    }
    let rid = get_u32(raw, 4);
    if state.resources.region(rid).is_none() {
        send_error(c, errcode::BAD_VALUE, seq, rid, 130, 11, "XFIXES SetRegion bad rid");
        return true;
    }
    let rects = xfixes_parse_rects(raw, 8);
    let n = rects.len();
    let new_region = Region::from_rects(rects);
    *state.resources.region_mut(rid).unwrap() = new_region;
    log::rep(c.id, seq, format_args!("XFIXES SetRegion {rid:#x} n={n}"), &[]);
    true
}

// ── CopyRegion (12) ────────────────────────────────────────────────
//     dword 4: source
//     dword 8: destination
fn handle_xfixes_copy_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 12, "XFIXES CopyRegion short");
        return true;
    }
    let src = get_u32(raw, 4);
    let dst = get_u32(raw, 8);
    let copy = match xfixes_get_region(state, src) {
        Some(r) => r,
        None => {
            send_error(c, errcode::BAD_VALUE, seq, src, 130, 12, "XFIXES CopyRegion bad src");
            return true;
        }
    };
    match state.resources.region_mut(dst) {
        Some(r) => { *r = copy; }
        None => {
            send_error(c, errcode::BAD_VALUE, seq, dst, 130, 12, "XFIXES CopyRegion bad dst");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!("XFIXES CopyRegion {src:#x} → {dst:#x}"), &[]);
    true
}

// Common layout for UnionRegion, IntersectRegion, SubtractRegion:
//     dword 4: source1
//     dword 8: source2
//     dword 12: destination
fn xfixes_combine_regions(
    c: &mut Client,
    state: &mut ServerState,
    seq: u16,
    minor: u16,
    raw: &[u8],
    f: impl Fn(&Region, &Region) -> Region,
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, minor, "XFIXES region combine short");
        return true;
    }
    let s1 = get_u32(raw, 4);
    let s2 = get_u32(raw, 8);
    let dst = get_u32(raw, 12);
    let Some(a) = xfixes_get_region(state, s1) else {
        send_error(c, errcode::BAD_VALUE, seq, s1, 130, minor, "bad source1");
        return true;
    };
    let Some(b) = xfixes_get_region(state, s2) else {
        send_error(c, errcode::BAD_VALUE, seq, s2, 130, minor, "bad source2");
        return true;
    };
    let out = f(&a, &b);
    match state.resources.region_mut(dst) {
        Some(r) => { *r = out; }
        None => {
            send_error(c, errcode::BAD_VALUE, seq, dst, 130, minor, "bad dst");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!("XFIXES region combine {s1:#x},{s2:#x} → {dst:#x}"), &[]);
    true
}

fn handle_xfixes_union_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    xfixes_combine_regions(c, state, seq, 13, raw, |a, b| a.union(b))
}

fn handle_xfixes_intersect_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    xfixes_combine_regions(c, state, seq, 14, raw, |a, b| a.intersect(b))
}

fn handle_xfixes_subtract_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    xfixes_combine_regions(c, state, seq, 15, raw, |a, b| a.subtract(b))
}

// ── InvertRegion (16) ──────────────────────────────────────────────
//     dword 4: source
//     word 8:  bounds_x  word 10: bounds_y
//     word 12: bounds_w  word 14: bounds_h
//     dword 16: destination
fn handle_xfixes_invert_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 20 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 16, "XFIXES InvertRegion short");
        return true;
    }
    let src = get_u32(raw, 4);
    let bx = get_u16(raw, 8) as i16;
    let by = get_u16(raw, 10) as i16;
    let bw = get_u16(raw, 12);
    let bh = get_u16(raw, 14);
    let dst = get_u32(raw, 16);
    let Some(a) = xfixes_get_region(state, src) else {
        send_error(c, errcode::BAD_VALUE, seq, src, 130, 16, "bad src");
        return true;
    };
    let inv = a.invert(RegRect::new(bx, by, bw, bh));
    match state.resources.region_mut(dst) {
        Some(r) => { *r = inv; }
        None => {
            send_error(c, errcode::BAD_VALUE, seq, dst, 130, 16, "bad dst");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!(
        "XFIXES InvertRegion {src:#x} bounds={bw}x{bh}+{bx}+{by} → {dst:#x}"
    ), &[]);
    true
}

// ── TranslateRegion (17) ───────────────────────────────────────────
//     dword 4: region
//     word 8: dx   word 10: dy
fn handle_xfixes_translate_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 17, "XFIXES TranslateRegion short");
        return true;
    }
    let rid = get_u32(raw, 4);
    let dx = get_u16(raw, 8) as i16;
    let dy = get_u16(raw, 10) as i16;
    match state.resources.region_mut(rid) {
        Some(r) => r.translate(dx, dy),
        None => {
            send_error(c, errcode::BAD_VALUE, seq, rid, 130, 17, "bad rid");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!("XFIXES TranslateRegion {rid:#x} +{dx}+{dy}"), &[]);
    true
}

// ── RegionExtents (18) ─────────────────────────────────────────────
//     dword 4: source
//     dword 8: destination
// Destination region becomes a single rect covering source.extents().
fn handle_xfixes_region_extents(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 12 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 18, "XFIXES RegionExtents short");
        return true;
    }
    let src = get_u32(raw, 4);
    let dst = get_u32(raw, 8);
    let ext = match xfixes_get_region(state, src) {
        Some(r) => r.extents(),
        None => {
            send_error(c, errcode::BAD_VALUE, seq, src, 130, 18, "bad src");
            return true;
        }
    };
    match state.resources.region_mut(dst) {
        Some(r) => {
            *r = if ext.is_empty() { Region::empty() } else { Region::from_rect(ext) };
        }
        None => {
            send_error(c, errcode::BAD_VALUE, seq, dst, 130, 18, "bad dst");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!(
        "XFIXES RegionExtents {src:#x} → {dst:#x} {}x{}+{}+{}",
        ext.w, ext.h, ext.x, ext.y,
    ), &[]);
    true
}

// ── FetchRegion (19) ───────────────────────────────────────────────
// Reply body:
//   dword 4: length (patched)
//   word 8: extents_x  word 10: extents_y
//   word 12: extents_w word 14: extents_h
//   16 pad bytes
//   LISTofRECTANGLE (8 bytes each)
fn handle_xfixes_fetch_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 8 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 19, "XFIXES FetchRegion short");
        return true;
    }
    let rid = get_u32(raw, 4);
    let region = match xfixes_get_region(state, rid) {
        Some(r) => r,
        None => {
            send_error(c, errcode::BAD_VALUE, seq, rid, 130, 19, "bad rid");
            return true;
        }
    };
    let ext = region.extents();
    let mut out = Vec::new();
    put_reply_header(&mut out, seq, 0, 0);   // patch length below
    put_u16(&mut out, ext.x as u16);
    put_u16(&mut out, ext.y as u16);
    put_u16(&mut out, ext.w);
    put_u16(&mut out, ext.h);
    for _ in 0..16 { out.push(0); }
    debug_assert_eq!(out.len(), 32);
    for r in &region.rects {
        put_u16(&mut out, r.x as u16);
        put_u16(&mut out, r.y as u16);
        put_u16(&mut out, r.w);
        put_u16(&mut out, r.h);
    }
    while out.len() % 4 != 0 { out.push(0); }
    let extra = ((out.len() - 32) / 4) as u32;
    out[4..8].copy_from_slice(&extra.to_le_bytes());
    c.write_buf.extend_from_slice(&out);
    log::rep(c.id, seq, format_args!(
        "XFIXES FetchRegion {rid:#x} → n={} extents={}x{}+{}+{}",
        region.rects.len(), ext.w, ext.h, ext.x, ext.y
    ), &[]);
    true
}

// ── SetGCClipRegion (20) ───────────────────────────────────────────
//     dword 4: gc
//     word 8: clip_x   word 10: clip_y
//     dword 12: region (0 = none)
fn handle_xfixes_set_gc_clip_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 20, "XFIXES SetGCClipRegion short");
        return true;
    }
    let gid = get_u32(raw, 4);
    let clip_x = get_u16(raw, 8) as i16;
    let clip_y = get_u16(raw, 10) as i16;
    let rid = get_u32(raw, 12);
    let rects: Vec<(i16, i16, u16, u16)> = if rid == 0 {
        Vec::new()
    } else {
        match xfixes_get_region(state, rid) {
            Some(r) => r.rects.iter().map(|r| (r.x, r.y, r.w, r.h)).collect(),
            None => {
                send_error(c, errcode::BAD_VALUE, seq, rid, 130, 20, "bad region");
                return true;
            }
        }
    };
    match state.resources.gc_mut(gid) {
        Some(g) => {
            g.clip_x_origin = clip_x;
            g.clip_y_origin = clip_y;
            g.clip_rects = rects;
        }
        None => {
            send_error(c, errcode::BAD_GCONTEXT, seq, gid, 130, 20, "bad gc");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!(
        "XFIXES SetGCClipRegion gc={gid:#x} region={rid:#x}"
    ), &[]);
    true
}

// ── SetCursorName (23) — stub ──────────────────────────────────
fn handle_xfixes_set_cursor_name_stub(
    c: &mut Client, _state: &mut ServerState, seq: u16, _raw: &[u8],
) -> bool {
    log::rep(c.id, seq, format_args!("XFIXES SetCursorName (stub)"), &[]);
    true
}

// ── SetPictureClipRegion (22) ──────────────────────────────────────
//     dword 4: picture
//     word 8: clip_x  word 10: clip_y
//     dword 12: region (0 = none)
fn handle_xfixes_set_picture_clip_region(
    c: &mut Client, state: &mut ServerState, seq: u16, raw: &[u8],
) -> bool {
    if raw.len() < 16 {
        send_error(c, errcode::BAD_LENGTH, seq, 0, 130, 22, "XFIXES SetPictureClipRegion short");
        return true;
    }
    let pid = get_u32(raw, 4);
    let clip_x = get_u16(raw, 8) as i16;
    let clip_y = get_u16(raw, 10) as i16;
    let rid = get_u32(raw, 12);
    let new_region: Option<Region> = if rid == 0 {
        None
    } else {
        match xfixes_get_region(state, rid) {
            Some(mut r) => {
                r.translate(clip_x, clip_y);
                Some(r)
            }
            None => {
                send_error(c, errcode::BAD_VALUE, seq, rid, 130, 22, "bad region");
                return true;
            }
        }
    };
    match state.resources.picture_mut(pid) {
        Some(p) => { p.clip_region = new_region; }
        None => {
            send_error(c, errcode::BAD_VALUE, seq, pid, 130, 22, "bad picture");
            return true;
        }
    }
    log::rep(c.id, seq, format_args!(
        "XFIXES SetPictureClipRegion pic={pid:#x} region={rid:#x}"
    ), &[]);
    true
}

// Silence transient unused-import warnings.
#[allow(unused_imports)]
use atom as _;
#[allow(unused_imports)]
use ResourceMap as _;
