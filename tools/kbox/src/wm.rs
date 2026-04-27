// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// `become_wm` — perform the eight X11 requests that satisfy
// `xprop -root _NET_SUPPORTING_WM_CHECK` and register kbox as the
// active window manager per EWMH §3.

use std::io;

use crate::conn::X11Conn;
use crate::reply::{
    await_reply, parse_get_geometry_reply, parse_get_property_reply,
    parse_get_window_attributes_reply, parse_intern_atom_reply,
    parse_list_properties_reply, parse_query_extension_reply,
    parse_query_tree_reply, Frame,
};
use crate::req::{
    alloc_named_color, build_client_message, change_property_append_string,
    change_property_string, change_property_window,
    change_window_attrs_cursor, change_window_attrs_event_mask, configure_window,
    create_gc_default, create_glyph_cursor, create_pixmap,
    create_window_input_only, flush, free_gc,
    free_pixmap, get_geometry, get_keyboard_mapping, get_property,
    get_window_attributes, grab_button, grab_key, intern_atom, list_properties,
    map_window, open_font, query_extension, query_tree, send_event, set_input_focus,
    set_selection_owner, shm_attach, shm_query_version,
    xkb_get_controls, xkb_get_names, xkb_get_state, xkb_use_extension,
    EVT_BUTTON_PRESS, EVT_BUTTON_RELEASE,
    EVT_SUBSTRUCTURE_NOTIFY, EVT_SUBSTRUCTURE_REDIRECT, XA_WINDOW,
};
use crate::{err_, info, kbox_log, log};

pub struct WmState {
    pub check_window: u32,
    pub atom_wm_s0: u32,
    pub atom_net_supporting_wm_check: u32,
    pub atom_net_wm_name: u32,
    pub atom_utf8_string: u32,
}

pub fn become_wm(conn: &mut X11Conn) -> io::Result<WmState> {
    info!("becoming WM on root=0x{:x}", conn.info.root_xid);

    // ── 1. Intern all four atoms in one batch ────────────────────────
    //
    // We send all four InternAtom requests, flush, then read four
    // replies in order.  X11 guarantees replies come back in
    // request-sequence order on the same connection.
    let mut out = Vec::with_capacity(256);
    let s_wm_s0  = intern_atom(&mut out, conn, "WM_S0", false);
    let s_check  = intern_atom(&mut out, conn, "_NET_SUPPORTING_WM_CHECK", false);
    let s_name   = intern_atom(&mut out, conn, "_NET_WM_NAME", false);
    let s_utf8   = intern_atom(&mut out, conn, "UTF8_STRING", false);
    flush(&mut out, conn)?;

    let atom_wm_s0 = expect_atom(await_reply(conn, s_wm_s0)?,  "WM_S0")?;
    let atom_check = expect_atom(await_reply(conn, s_check)?,  "_NET_SUPPORTING_WM_CHECK")?;
    let atom_name  = expect_atom(await_reply(conn, s_name)?,   "_NET_WM_NAME")?;
    let atom_utf8  = expect_atom(await_reply(conn, s_utf8)?,   "UTF8_STRING")?;

    // ── 2. Create the EWMH check window ──────────────────────────────
    let (_s_cw, check_window) = create_window_input_only(
        &mut out, conn, conn.info.root_xid, conn.info.root_visual,
    );

    // ── 3. Claim WM_S0 selection ─────────────────────────────────────
    let _s_ssel = set_selection_owner(&mut out, conn, check_window, atom_wm_s0);

    // ── 4–6. Set the EWMH properties ─────────────────────────────────
    let _s_p1 = change_property_window(&mut out, conn, conn.info.root_xid,
                                       atom_check, check_window);
    let _s_p2 = change_property_window(&mut out, conn, check_window,
                                       atom_check, check_window);
    let _s_p3 = change_property_string(&mut out, conn, check_window,
                                       atom_name, atom_utf8, b"kbox");

    flush(&mut out, conn)?;

    // None of CreateWindow/SetSelectionOwner/ChangeProperty has a
    // reply.  Errors come back asynchronously — they'll appear in
    // the post-init drain loop in main.rs.

    let _ = XA_WINDOW; // silence unused-import lint

    info!("WM setup complete: check_window=0x{:x} atoms wm_s0={} check={} wm_name={} utf8={}",
          check_window, atom_wm_s0, atom_check, atom_name, atom_utf8);

    Ok(WmState {
        check_window,
        atom_wm_s0,
        atom_net_supporting_wm_check: atom_check,
        atom_net_wm_name: atom_name,
        atom_utf8_string: atom_utf8,
    })
}

// ─── Phase 1 ────────────────────────────────────────────────────────────
// SubstructureRedirect grab on root.  This is the WM "I am here"
// claim — once a client holds it, all MapRequest/ConfigureRequest
// events for top-level windows route to *that* client instead of
// the server's default.  Only one client can hold it; a second
// attempt returns BadAccess.
//
// Hypothesis under test: if our event-mask demux on root has a
// subtle off-by-one (or our wake_all on POLL_WAIT_QUEUE drops
// SubstructureNotify edges on the floor), this grab triggers the
// hang.

pub fn phase1_substructure_redirect(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 1 entry: ChangeWindowAttributes(root, SubstructureRedirect|SubstructureNotify)");
    let mut out = Vec::with_capacity(64);
    let mask = EVT_SUBSTRUCTURE_REDIRECT | EVT_SUBSTRUCTURE_NOTIFY;
    let _seq = change_window_attrs_event_mask(
        &mut out, conn, conn.info.root_xid, mask,
    );
    flush(&mut out, conn)?;
    info!("PHASE 1 done: substructure redirect grab issued");
    Ok(())
}

// ─── Phase 2-7 stubs ────────────────────────────────────────────────────
// Each is a no-op until we get past the prior phase.  When a phase
// is implemented, replace its body; the dispatcher in main.rs already
// calls them in order.

// ─── Phase 2 ────────────────────────────────────────────────────────────
// Window enumeration: QueryTree(root) → list of children →
// GetWindowAttributes + GetGeometry for each.  This is what openbox
// does at startup to "adopt" pre-existing top-level windows.
//
// Hypothesis: variable-length reply parsing on QueryTree (children
// list) or some attribute-byte ordering surface a kernel-side
// AF_UNIX read-buffer alignment bug under repeated reply traffic.

pub fn phase2_query_tree(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 2 entry: QueryTree + GetWindowAttributes + GetGeometry");
    let mut out = Vec::with_capacity(64);
    let s_qt = query_tree(&mut out, conn, conn.info.root_xid);
    flush(&mut out, conn)?;

    let qt_frame = await_reply(conn, s_qt)?;
    let (root, parent, children) = match parse_query_tree_reply(&qt_frame) {
        Some(t) => t,
        None => {
            err_!("QueryTree reply parse failed");
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                                      "QueryTree reply"));
        }
    };
    info!("QueryTree(root) → root=0x{:x} parent=0x{:x} {} children",
          root, parent, children.len());

    // For each child: GetWindowAttributes + GetGeometry.  Batch all
    // requests then drain replies in order.
    let mut seqs = Vec::with_capacity(children.len() * 2);
    for &child in &children {
        let s_a = get_window_attributes(&mut out, conn, child);
        let s_g = get_geometry(&mut out, conn, child);
        seqs.push((child, s_a, s_g));
    }
    flush(&mut out, conn)?;

    for (child, s_a, s_g) in &seqs {
        let attrs = await_reply(conn, *s_a)?;
        let geom  = await_reply(conn, *s_g)?;
        let map_state = parse_get_window_attributes_reply(&attrs);
        let g = parse_get_geometry_reply(&geom);
        info!("child=0x{:x} map_state={:?} geom={:?}", child, map_state, g);
    }
    info!("PHASE 2 done: {} children inspected", children.len());
    Ok(())
}
// ─── Phase 3 ────────────────────────────────────────────────────────────
// Read every property currently set on the root window.  openbox does
// this to pick up state already published by other clients (display
// resolution, working area, prior WM's _NET_CURRENT_DESKTOP, etc.).
//
// Hypothesis: GetProperty's variable-length value-data trailer (with
// padding to 4 bytes) exercises a different code path in our
// AF_UNIX read pump than fixed-size replies.

pub fn phase3_root_properties(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 3 entry: ListProperties + GetProperty for every root atom");
    let mut out = Vec::with_capacity(64);
    let s_lp = list_properties(&mut out, conn, conn.info.root_xid);
    flush(&mut out, conn)?;
    let lp = await_reply(conn, s_lp)?;
    let atoms = parse_list_properties_reply(&lp).unwrap_or_default();
    info!("ListProperties(root) → {} atoms", atoms.len());

    let mut seqs = Vec::with_capacity(atoms.len());
    for &atom in &atoms {
        let s = get_property(&mut out, conn, conn.info.root_xid, atom);
        seqs.push((atom, s));
    }
    flush(&mut out, conn)?;

    for (atom, s) in &seqs {
        let f = await_reply(conn, *s)?;
        let v = parse_get_property_reply(&f);
        match v {
            Some(r) => info!("  prop atom=0x{:x} type=0x{:x} fmt={} bytes={}",
                             atom, r.type_, r.format, r.value.len()),
            None    => info!("  prop atom=0x{:x} (error or non-reply)", atom),
        }
    }
    info!("PHASE 3 done: {} root properties read", atoms.len());
    Ok(())
}
// ─── Phase 4 ────────────────────────────────────────────────────────────
// Keyboard mapping query + a handful of passive key grabs.  openbox
// installs grabs for Mod1+Tab (window switcher), Mod1+F4 (close),
// Mod4+e (file manager), etc.  GrabKey doesn't reply on success
// but issues a server-side error frame on collision.
//
// Hypothesis: GrabKey takes a long lock or reads keymap state in
// the kernel's input path; if our /dev/input layer mishandles the
// access it could trip the hang.

pub fn phase4_keyboard_grabs(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 4 entry: GetKeyboardMapping + GrabKey");
    let mut out = Vec::with_capacity(64);

    // Pull the keyboard mapping for the standard 8..255 range.
    // We only inspect the reply size; we don't actually map keysyms
    // here — for the bisect we just want to exercise the request.
    let s_km = get_keyboard_mapping(&mut out, conn, 8, 248);
    flush(&mut out, conn)?;
    let km = await_reply(conn, s_km)?;
    if let Frame::Reply { extra, .. } = &km {
        info!("GetKeyboardMapping → {} bytes", extra.len());
    }

    // Grab a few common WM combos on root, AnyModifier, async modes.
    // 0x8000 = AnyModifier; pointer/keyboard mode 1 = Async.
    let grabs: &[u8] = &[
        0x09, // ESC
        0x17, // TAB
        0x47, // F4 (Mod1+F4 = close)
        0x4f, // F8
        0x71, // Left
        0x72, // Right
        0x6F, // Up
        0x74, // Down
        0x40, // Alt-L
        0x6c, // Alt-R
    ];
    for &k in grabs {
        let _s = grab_key(&mut out, conn, false, conn.info.root_xid,
                          0x8000 /* AnyModifier */, k, 1, 1);
    }
    flush(&mut out, conn)?;

    info!("PHASE 4 done: {} GrabKey requests issued", grabs.len());
    Ok(())
}
// ─── Phase 5 ────────────────────────────────────────────────────────────
// Passive button grabs on root for click-to-focus / click-to-raise.
// AnyButton + AnyModifier covers everything; sync mode for both.
pub fn phase5_button_grabs(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 5 entry: GrabButton (button1/2/3, AnyModifier)");
    let mut out = Vec::with_capacity(64);
    let mask = (EVT_BUTTON_PRESS | EVT_BUTTON_RELEASE) as u16;
    for button in &[1u8, 2, 3] {
        let _ = grab_button(&mut out, conn, false, conn.info.root_xid,
                            mask, 1 /*async pointer*/, 1 /*async kbd*/,
                            0 /*confine_to=None*/, 0 /*cursor=None*/,
                            *button, 0x8000 /*AnyModifier*/);
    }
    flush(&mut out, conn)?;
    info!("PHASE 5 done: 3 GrabButton requests issued");
    Ok(())
}

// ─── Phase 6 ────────────────────────────────────────────────────────────
// SetInputFocus(PointerRoot, CurrentTime).  Establishes focus
// follows pointer.  This triggers FocusIn/FocusOut events to be
// generated on relevant windows; if our event delivery has a bug
// it would surface here.
pub fn phase6_focus(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 6 entry: SetInputFocus(PointerRoot, CurrentTime)");
    let mut out = Vec::with_capacity(32);
    // focus = 1 means PointerRoot per X11 spec.
    let _ = set_input_focus(&mut out, conn, 1 /*RevertToPointerRoot*/,
                            1 /*PointerRoot*/, 0 /*CurrentTime*/);
    flush(&mut out, conn)?;
    info!("PHASE 6 done: focus set to PointerRoot");
    Ok(())
}
// ─── Phase 7 ────────────────────────────────────────────────────────────
// Tight client-side polling loop, mimicking openbox/libev's
// pattern: every ~5ms, fire a GetProperty on root for a known
// atom, drain the reply, repeat.  This puts a continuous request/
// reply stream on the wire that overlaps any other client's
// connect+request work.  Strongest match for the hang signature
// from blog 231 ("xprop never accepted while busy WM is connected").
//
// Runs for ~30 seconds — long enough for the test's xprop probe to
// land in the middle of the busy phase.  Then returns; main.rs
// puts kbox back in the slow idle loop.

// ─── Phase 19 ───────────────────────────────────────────────────────────
// kxreplay bisected the trigger to a 3-request sequence:
//   1. OpenFont(font="cursor")
//   2. CreateGlyphCursor(cid, source=font, mask=font, src_char=68, mask_char=69)
//   3. ChangeWindowAttributes(root, CW_CURSOR=cid)
// Replaying just these three requests (with no openbox setup
// preceding them) hangs Xorg the same way as full openbox: xprop
// blocks for 30s+.  Phase 19 issues exactly this triplet and
// becomes the kernel-bug minimal C-equivalent reproducer.
//
// KBOX_PHASE_19_VARIANT env var picks an ablation:
//   "" / "all" → full triplet (default)
//   "noset"    → OpenFont + CreateGlyphCursor only (no CW_CURSOR)
//   "noglyph"  → OpenFont + CW_CURSOR with the (uncreated) XID
//   "nofont"   → CreateGlyphCursor with junk font + CW_CURSOR
pub fn phase19_minimal_cursor_trigger(conn: &mut X11Conn) -> io::Result<()> {
    let variant = std::env::var("KBOX_PHASE_19_VARIANT").unwrap_or_default();
    info!("PHASE 19 entry variant={:?}", variant);
    let mut out = Vec::with_capacity(128);

    match variant.as_str() {
        "noset" => {
            let (_, fid) = open_font(&mut out, conn, "cursor");
            let (_, cid) = create_glyph_cursor(&mut out, conn, fid, fid,
                68, 69, (0, 0, 0), (0xffff, 0xffff, 0xffff));
            info!("phase19/noset fid=0x{:x} cid=0x{:x} (no CW_CURSOR)", fid, cid);
        }
        "noglyph" => {
            let (_, fid) = open_font(&mut out, conn, "cursor");
            let bogus_cid = conn.alloc_xid();
            let _ = change_window_attrs_cursor(&mut out, conn,
                conn.info.root_xid, bogus_cid);
            info!("phase19/noglyph fid=0x{:x} CW_CURSOR(root, bogus=0x{:x})", fid, bogus_cid);
        }
        "nofont" => {
            let bogus_fid = conn.alloc_xid();
            let (_, cid) = create_glyph_cursor(&mut out, conn, bogus_fid, bogus_fid,
                68, 69, (0, 0, 0), (0xffff, 0xffff, 0xffff));
            let _ = change_window_attrs_cursor(&mut out, conn, conn.info.root_xid, cid);
            info!("phase19/nofont (junk font 0x{:x}) cid=0x{:x}", bogus_fid, cid);
        }
        _ => {
            let (_, fid) = open_font(&mut out, conn, "cursor");
            let (_, cid) = create_glyph_cursor(&mut out, conn, fid, fid,
                68, 69, (0, 0, 0), (0xffff, 0xffff, 0xffff));
            let _ = change_window_attrs_cursor(&mut out, conn, conn.info.root_xid, cid);
            info!("phase19/all fid=0x{:x} cid=0x{:x} CW_CURSOR(root, cid)", fid, cid);
        }
    }
    flush(&mut out, conn)?;
    info!("PHASE 19 done");
    Ok(())
}

// ─── Phase 18 ───────────────────────────────────────────────────────────
// kxreplay (task #41) bisected the openbox-trigger to chunk #137,
// bytes 0-15: ChangeWindowAttributes(root, CW_CURSOR=0x00200002).
// Phase 18 issues that exact request (with kbox's own allocated
// XID for the cursor argument) and sees if it reproduces.
//
// Note: cursor=0 is "None" (no cursor); to actually exercise the
// CW_CURSOR path we need a non-zero XID.  Try both — if either
// hangs, we have the trigger.

pub fn phase18_cursor_change(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 18 entry: ChangeWindowAttributes(root, CW_CURSOR=...)");
    let mut out = Vec::with_capacity(64);

    // 18a. Try with a bogus cursor XID — Xorg returns BadCursor
    //      asynchronously, but we want to see if the request
    //      itself triggers the hang.
    let bogus_xid = conn.alloc_xid();  // unallocated XID
    let _ = change_window_attrs_cursor(&mut out, conn,
                                       conn.info.root_xid, bogus_xid);
    flush(&mut out, conn)?;
    info!("phase18 sent CW_CURSOR=0x{:x} (bogus, expect BadCursor)", bogus_xid);

    // Sleep briefly so Xorg's async error has a chance to
    // surface, then try with cursor=None (= 0, no cursor).
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = change_window_attrs_cursor(&mut out, conn,
                                       conn.info.root_xid, 0);
    flush(&mut out, conn)?;
    info!("phase18 sent CW_CURSOR=0 (None)");

    info!("PHASE 18 done");
    Ok(())
}

// ─── Phase 17 ───────────────────────────────────────────────────────────
// Replay openbox's last C2S chunk before Xorg stopped responding.
// kxproxy (blog 239) captured a 96-byte sequence containing:
//   1. SendEvent → ClientMessage to root, mask=SubstructureNotify,
//      message-type=_NET_STARTUP_INFO_BEGIN, data="wm started\0...".
//   2. ChangeProperty(Append) on a child window, property=WM_CLASS,
//      type=STRING, format=8.
//
// Phase 17 issues exactly that pair, looped for 30s, after the
// normal WM setup.  If the kernel divergence is in either of these
// two requests, kbox phase 17 reproduces the hang and we have a
// kernel-bug repro that's 100% in our source.

pub fn phase17_replay_openbox_trigger(conn: &mut X11Conn) -> io::Result<()> {
    use std::time::{Duration, Instant};
    info!("PHASE 17 entry: replay openbox's last 96-byte chunk (SendEvent + ChangeProperty Append)");

    // 1. Intern the atoms openbox used in #276:
    //    - _NET_STARTUP_INFO_BEGIN  (the ClientMessage type)
    //    - WM_CLASS                  (the property atom for the Append)
    //    - STRING                    (the property TYPE — predefined atom 31, no intern needed)
    let mut out = Vec::with_capacity(256);
    let s_sib   = intern_atom(&mut out, conn, "_NET_STARTUP_INFO_BEGIN", false);
    let s_wmcls = intern_atom(&mut out, conn, "WM_CLASS", false);
    flush(&mut out, conn)?;
    let atom_sib   = expect_atom(await_reply(conn, s_sib)?,   "_NET_STARTUP_INFO_BEGIN")?;
    let atom_wmcls = expect_atom(await_reply(conn, s_wmcls)?, "WM_CLASS")?;
    info!("phase17 atoms: _NET_STARTUP_INFO_BEGIN=0x{:x} WM_CLASS=0x{:x}",
          atom_sib, atom_wmcls);

    // 2. Create a child window we can change properties on.
    //    InputOnly is fine — WM_CLASS is metadata, not visible.
    let (_s_cw, child) = create_window_input_only(
        &mut out, conn, conn.info.root_xid, conn.info.root_visual);
    flush(&mut out, conn)?;

    // 3. The exact 20-byte ClientMessage payload openbox sent.  We
    //    don't know the trailing 6 bytes after "wm started" from
    //    openbox's strace beyond what kxproxy logged — and those
    //    bytes look like uninitialised stack data ("c8 d6 14 10 12
    //    02 06 00") which means openbox made a 14-byte string look
    //    like 20 bytes by leaving the tail uninitialised.  We
    //    reproduce that exactly: 14 bytes "wm started\0\0\0\0" plus
    //    6 bytes of arbitrary content.
    let mut ev_data = [0u8; 20];
    let s = b"wm started\0";
    ev_data[..s.len()].copy_from_slice(s);
    // Trailing 6 bytes — any non-zero pattern; mimics openbox's
    // uninitialised tail.
    ev_data[14..20].copy_from_slice(&[0xc8, 0xd6, 0x14, 0x10, 0x12, 0x02]);

    // 4. Loop the SendEvent + ChangeProperty Append for 30s so the
    //    test's xprop probe lands mid-burst.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut sent = 0u64;
    while Instant::now() < deadline {
        // SendEvent ClientMessage to root with SubstructureNotify mask.
        let event = build_client_message(
            8, conn.info.root_xid, atom_sib, &ev_data);
        let _ = send_event(&mut out, conn, false,
                           conn.info.root_xid,
                           EVT_SUBSTRUCTURE_NOTIFY,
                           &event);
        // Append a small WM_CLASS-style STRING to the child window.
        let _ = change_property_append_string(
            &mut out, conn, child, atom_wmcls,
            crate::req::XA_STRING, b"kbox.kbox\0");
        flush(&mut out, conn)?;
        sent += 2;
    }
    info!("PHASE 17 done: sent {} SendEvent+ChangeProperty pairs in 30s", sent);
    Ok(())
}

// ─── Phase 16 ───────────────────────────────────────────────────────────
// Phase 15 with 3× the filesystem load: spawn 3 readdir-storm
// threads instead of 1.  Phase 15's xprop took 2s (vs 0s in
// every prior phase) — we're close to the threshold.  Triple
// the FS load to push past it.

pub fn phase16_triple_fs(conn: &mut X11Conn) -> io::Result<()> {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    info!("PHASE 16 entry: 3× FS storm + threaded X11 burst");

    let read_sock = conn.sock.try_clone()?;
    let frames_seen = Arc::new(AtomicU32::new(0));
    let bytes_seen  = Arc::new(AtomicU32::new(0));
    let dir_reads   = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    let f_w = frames_seen.clone();
    let b_w = bytes_seen.clone();
    let s_w = stop.clone();
    let xworker = std::thread::spawn(move || {
        use std::io::Read;
        let mut sock = read_sock;
        let mut buf = [0u8; 4096];
        loop {
            if s_w.load(Ordering::Relaxed) { break; }
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    b_w.fetch_add(n as u32, Ordering::Relaxed);
                    f_w.fetch_add((n as u32 + 31) / 32, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    let mut fsworkers = Vec::new();
    for _ in 0..3 {
        let dr = dir_reads.clone();
        let sf = stop.clone();
        fsworkers.push(std::thread::spawn(move || {
            let dirs: &[&[u8]] = &[
                b"/usr/share/fonts\0",
                b"/usr/share/fonts/misc\0",
                b"/usr/share/X11\0",
                b"/etc\0",
                b"/usr/lib\0",
                b"/usr/share\0",
                b"/usr/share/icons\0",
                b"/root\0",
                b"/root/.config\0",
                b"/root/.config/openbox\0",
                b"/usr/bin\0",
                b"/usr/sbin\0",
                b"/lib\0",
            ];
            loop {
                if sf.load(Ordering::Relaxed) { break; }
                for path in dirs {
                    let fd = unsafe {
                        libc::open(path.as_ptr() as *const libc::c_char,
                                   libc::O_RDONLY | libc::O_DIRECTORY)
                    };
                    if fd < 0 { continue; }
                    let mut buf = [0u8; 1024];
                    loop {
                        let n = unsafe {
                            libc::syscall(libc::SYS_getdents64,
                                          fd as libc::c_long,
                                          buf.as_mut_ptr() as libc::c_long,
                                          buf.len() as libc::c_long)
                        };
                        if n <= 0 { break; }
                        dr.fetch_add(1, Ordering::Relaxed);
                    }
                    unsafe { libc::close(fd); }
                    if sf.load(Ordering::Relaxed) { break; }
                }
            }
        }));
    }

    let mut out = Vec::with_capacity(512);
    let mut writes = 0u64;
    let deadline = Instant::now() + Duration::from_secs(30);
    const STREAM: &[&str] = &[
        "_NET_SUPPORTED", "_NET_NUMBER_OF_DESKTOPS", "_NET_DESKTOP_NAMES",
        "_NET_CURRENT_DESKTOP", "_NET_ACTIVE_WINDOW", "_NET_WORKAREA",
        "_NET_CLIENT_LIST", "_NET_FRAME_EXTENTS", "WM_PROTOCOLS",
        "WM_DELETE_WINDOW", "WM_TAKE_FOCUS", "WM_STATE",
        "_MOTIF_WM_HINTS", "_NET_WM_STATE", "_NET_WM_DESKTOP",
        "_NET_WM_NAME", "_NET_WM_ICON", "_NET_WM_PID",
    ];
    while Instant::now() < deadline {
        for name in STREAM {
            out.clear();
            let _ = intern_atom(&mut out, conn, name, false);
            flush(&mut out, conn)?;
            writes += 1;
        }
    }

    stop.store(true, Ordering::Relaxed);
    use std::os::unix::io::AsRawFd;
    let _ = unsafe { libc::shutdown(conn.sock.as_raw_fd(), libc::SHUT_RD) };
    let _ = xworker.join();
    for w in fsworkers { let _ = w.join(); }

    info!("PHASE 16 done: {} writes, {} X11 frames, {} bytes, {} getdents calls",
          writes,
          frames_seen.load(Ordering::Relaxed),
          bytes_seen.load(Ordering::Relaxed),
          dir_reads.load(Ordering::Relaxed));
    Ok(())
}

// ─── Phase 15 ───────────────────────────────────────────────────────────
// The kitchen sink: phase 14's threaded X11 burst + the
// concurrent filesystem readdir storm openbox actually does
// (570 dirent reads of 1024 bytes on fd=6, scanning fonts,
// themes, config dirs).  Combines AF_UNIX I/O with vfs/ext2 I/O
// from the same process, similar to openbox's runtime profile.
//
// Hypothesis: the bug is in the interaction between our
// scheduler waking up tasks blocked in AF_UNIX recvmsg and our
// ext2 read path.  Stress both simultaneously.

pub fn phase15_kitchen_sink(conn: &mut X11Conn) -> io::Result<()> {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    info!("PHASE 15 entry: threaded X11 burst + filesystem readdir storm");

    let read_sock = conn.sock.try_clone()?;
    let frames_seen = Arc::new(AtomicU32::new(0));
    let bytes_seen  = Arc::new(AtomicU32::new(0));
    let dir_reads   = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    let f_w = frames_seen.clone();
    let b_w = bytes_seen.clone();
    let s_w = stop.clone();

    let xworker = std::thread::spawn(move || {
        use std::io::Read;
        let mut sock = read_sock;
        let mut buf = [0u8; 4096];
        loop {
            if s_w.load(Ordering::Relaxed) { break; }
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    b_w.fetch_add(n as u32, Ordering::Relaxed);
                    f_w.fetch_add((n as u32 + 31) / 32, Ordering::Relaxed);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => break,
            }
        }
    });

    let dir_reads_fs = dir_reads.clone();
    let stop_fs = stop.clone();
    let fsworker = std::thread::spawn(move || {
        // Mirror openbox's "scan fonts/themes/config" pattern:
        // open a directory, read its entries via getdents, close,
        // repeat with a different directory.  Use libc directly
        // because std::fs::read_dir doesn't expose getdents
        // cadence cleanly.
        let dirs: &[&[u8]] = &[
            b"/usr/share/fonts\0",
            b"/usr/share/fonts/misc\0",
            b"/usr/share/X11\0",
            b"/etc\0",
            b"/usr/lib\0",
            b"/usr/share\0",
            b"/usr/share/icons\0",
            b"/root\0",
            b"/root/.config\0",
            b"/root/.config/openbox\0",
        ];
        loop {
            if stop_fs.load(Ordering::Relaxed) { break; }
            for path in dirs {
                let fd = unsafe {
                    libc::open(path.as_ptr() as *const libc::c_char,
                               libc::O_RDONLY | libc::O_DIRECTORY)
                };
                if fd < 0 { continue; }
                let mut buf = [0u8; 1024];
                loop {
                    let n = unsafe {
                        libc::syscall(libc::SYS_getdents64,
                                      fd as libc::c_long,
                                      buf.as_mut_ptr() as libc::c_long,
                                      buf.len() as libc::c_long)
                    };
                    if n <= 0 { break; }
                    dir_reads_fs.fetch_add(1, Ordering::Relaxed);
                }
                unsafe { libc::close(fd); }
                if stop_fs.load(Ordering::Relaxed) { break; }
            }
        }
    });

    // Main thread: write burst.
    let mut out = Vec::with_capacity(512);
    let mut writes = 0u64;
    let deadline = Instant::now() + Duration::from_secs(30);
    const STREAM: &[&str] = &[
        "_NET_SUPPORTED", "_NET_NUMBER_OF_DESKTOPS", "_NET_DESKTOP_NAMES",
        "_NET_CURRENT_DESKTOP", "_NET_ACTIVE_WINDOW", "_NET_WORKAREA",
        "_NET_CLIENT_LIST", "_NET_FRAME_EXTENTS", "WM_PROTOCOLS",
        "WM_DELETE_WINDOW", "WM_TAKE_FOCUS", "WM_STATE",
        "_MOTIF_WM_HINTS", "_NET_WM_STATE", "_NET_WM_DESKTOP",
        "_NET_WM_NAME", "_NET_WM_ICON", "_NET_WM_PID",
    ];
    while Instant::now() < deadline {
        for name in STREAM {
            out.clear();
            let _ = intern_atom(&mut out, conn, name, false);
            flush(&mut out, conn)?;
            writes += 1;
        }
    }

    stop.store(true, Ordering::Relaxed);
    use std::os::unix::io::AsRawFd;
    let _ = unsafe { libc::shutdown(conn.sock.as_raw_fd(), libc::SHUT_RD) };
    let _ = xworker.join();
    let _ = fsworker.join();

    info!("PHASE 15 done: {} writes, {} X11 frames, {} bytes, {} getdents calls",
          writes,
          frames_seen.load(Ordering::Relaxed),
          bytes_seen.load(Ordering::Relaxed),
          dir_reads.load(Ordering::Relaxed));
    Ok(())
}

// ─── Phase 14 ───────────────────────────────────────────────────────────
// Multi-threaded burst: spawn a worker thread that reads from a
// try_clone()'d X11 socket while the main thread writes requests
// on the same fd.  This is the only major dimension where kbox
// has differed from real openbox so far — libxcb spawns a worker
// thread and openbox's syscall trace shows 92 futex calls + 2
// clones confirming concurrent reader+writer.
//
// Hypothesis: Kevlar's AF_UNIX path may have a wait_queue race
// when one task blocks in recvmsg and another writes through the
// same socket — the same kind of pattern that previously surfaced
// the listener-starvation bug (blog 231).  We have the SMP
// threading suite passing 14/14 in isolation, but those are
// micro-benchmarks; this is the first time we combine threading
// + AF_UNIX I/O + an active second-process X11 server.

pub fn phase14_threaded_burst(conn: &mut X11Conn) -> io::Result<()> {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    info!("PHASE 14 entry: spawning libxcb-style worker thread");

    // Clone the socket fd for the worker.  Both fds point at the
    // same kernel socket; concurrent use is what we want.
    let mut read_sock = conn.sock.try_clone()?;

    // Counters the worker writes; main thread reads at the end.
    let frames_seen   = Arc::new(AtomicU32::new(0));
    let bytes_seen    = Arc::new(AtomicU32::new(0));
    let worker_done   = Arc::new(AtomicBool::new(false));

    let frames_w = frames_seen.clone();
    let bytes_w  = bytes_seen.clone();
    let done_w   = worker_done.clone();

    let worker = std::thread::spawn(move || {
        // Block in read() forever, draining frames until the socket
        // is shut down or main signals us to stop.
        use std::io::Read;
        let mut buf = [0u8; 4096];
        loop {
            match read_sock.read(&mut buf) {
                Ok(0) => break,            // EOF
                Ok(n) => {
                    bytes_w.fetch_add(n as u32, Ordering::Relaxed);
                    // Each frame is at least 32 bytes; rough count.
                    frames_w.fetch_add((n as u32 + 31) / 32, Ordering::Relaxed);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                       || e.raw_os_error() == Some(libc::EAGAIN) => {
                    continue;
                }
                Err(_) => break,
            }
            if done_w.load(Ordering::Relaxed) { break; }
        }
        done_w.store(true, Ordering::Relaxed);
    });

    // Main thread: 30-second burst of writes.  Don't await any
    // replies — the worker reads them.  This is exactly libxcb's
    // pattern: main thread queues + flushes, worker drains.
    let mut out = Vec::with_capacity(512);
    let mut writes = 0u64;
    let deadline = Instant::now() + Duration::from_secs(30);

    // Atom-name stream to fire on each iteration; cycle through.
    const STREAM: &[&str] = &[
        "_NET_SUPPORTED", "_NET_NUMBER_OF_DESKTOPS", "_NET_DESKTOP_NAMES",
        "_NET_CURRENT_DESKTOP", "_NET_ACTIVE_WINDOW", "_NET_WORKAREA",
        "_NET_CLIENT_LIST", "_NET_FRAME_EXTENTS",
        "WM_PROTOCOLS", "WM_DELETE_WINDOW", "WM_TAKE_FOCUS", "WM_STATE",
        "_MOTIF_WM_HINTS", "_NET_WM_STATE", "_NET_WM_DESKTOP",
        "_NET_WM_NAME", "_NET_WM_ICON", "_NET_WM_PID",
    ];

    while Instant::now() < deadline {
        for name in STREAM {
            // Issue an InternAtom, no flush of replies.  The worker
            // will collect them.
            out.clear();
            let _ = intern_atom(&mut out, conn, name, false);
            flush(&mut out, conn)?;
            writes += 1;
        }
        // No sleep — let the burst run as fast as the kernel allows.
    }

    // Tell the worker to stop and unblock its read by shutting the
    // read side of OUR copy of the socket.  shutdown(SHUT_RD) on
    // the worker's clone would also work; either way, the worker's
    // read returns 0 (EOF) once the kernel sees the shutdown.
    worker_done.store(true, Ordering::Relaxed);
    use std::os::unix::io::AsRawFd;
    let _ = unsafe { libc::shutdown(conn.sock.as_raw_fd(), libc::SHUT_RD) };
    let _ = worker.join();

    let f = frames_seen.load(Ordering::Relaxed);
    let b = bytes_seen.load(Ordering::Relaxed);
    info!("PHASE 14 done: main wrote {} requests; worker read {} bytes (~{} frames) in 30s",
          writes, b, f);
    Ok(())
}

// ─── Phase 13 ───────────────────────────────────────────────────────────
// Mimic openbox/libxcb's actual init burst shape:
//   • Set the X11 socket non-blocking.
//   • For each request: writev → recvmsg-loop-until-EAGAIN → ppoll →
//     recvmsg the actual reply.
//   • Issue ~100 small requests in a tight burst (1-2s wall time).
//
// The strace from real openbox showed 408 recvmsg (271 EAGAIN) + 555
// ppoll cycles + 139 small writevs.  Our prior phases (7, 8) lacked
// the EAGAIN-pump cycles entirely — they used blocking reads.  This
// phase produces the recvmsg→EAGAIN→ppoll→recvmsg pattern
// libxcb's _xcb_in_read_block does.

pub fn phase13_libxcb_burst(conn: &mut X11Conn) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    info!("PHASE 13 entry: openbox/libxcb-style burst (~100 reqs with EAGAIN pump)");

    // Set the socket to non-blocking so recvmsg returns EAGAIN
    // when no data is buffered.  fcntl F_SETFL with O_NONBLOCK.
    let fd = conn.sock.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }

    // The atom names openbox interns at startup — every _NET_* hint
    // plus the WM_* + _MOTIF_*  set.  These are real openbox atoms,
    // pulled from the openbox/openbox.c source.
    const ATOMS: &[&str] = &[
        "_NET_SUPPORTED",
        "_NET_NUMBER_OF_DESKTOPS",
        "_NET_DESKTOP_NAMES",
        "_NET_DESKTOP_GEOMETRY",
        "_NET_DESKTOP_VIEWPORT",
        "_NET_CURRENT_DESKTOP",
        "_NET_ACTIVE_WINDOW",
        "_NET_WORKAREA",
        "_NET_CLIENT_LIST",
        "_NET_CLIENT_LIST_STACKING",
        "_NET_CLOSE_WINDOW",
        "_NET_MOVERESIZE_WINDOW",
        "_NET_WM_MOVERESIZE",
        "_NET_REQUEST_FRAME_EXTENTS",
        "_NET_RESTACK_WINDOW",
        "_NET_SHOWING_DESKTOP",
        "_NET_DESKTOP_LAYOUT",
        "_NET_VIRTUAL_ROOTS",
        "_NET_FRAME_EXTENTS",
        "_NET_WM_VISIBLE_NAME",
        "_NET_WM_ICON_NAME",
        "_NET_WM_VISIBLE_ICON_NAME",
        "_NET_WM_DESKTOP",
        "_NET_WM_WINDOW_TYPE",
        "_NET_WM_WINDOW_TYPE_DESKTOP",
        "_NET_WM_WINDOW_TYPE_DOCK",
        "_NET_WM_WINDOW_TYPE_TOOLBAR",
        "_NET_WM_WINDOW_TYPE_MENU",
        "_NET_WM_WINDOW_TYPE_UTILITY",
        "_NET_WM_WINDOW_TYPE_SPLASH",
        "_NET_WM_WINDOW_TYPE_DIALOG",
        "_NET_WM_WINDOW_TYPE_NORMAL",
        "_NET_WM_STATE",
        "_NET_WM_STATE_MODAL",
        "_NET_WM_STATE_STICKY",
        "_NET_WM_STATE_MAXIMIZED_VERT",
        "_NET_WM_STATE_MAXIMIZED_HORZ",
        "_NET_WM_STATE_SHADED",
        "_NET_WM_STATE_SKIP_TASKBAR",
        "_NET_WM_STATE_SKIP_PAGER",
        "_NET_WM_STATE_HIDDEN",
        "_NET_WM_STATE_FULLSCREEN",
        "_NET_WM_STATE_ABOVE",
        "_NET_WM_STATE_BELOW",
        "_NET_WM_STATE_DEMANDS_ATTENTION",
        "_NET_WM_ALLOWED_ACTIONS",
        "_NET_WM_ACTION_MOVE",
        "_NET_WM_ACTION_RESIZE",
        "_NET_WM_ACTION_MINIMIZE",
        "_NET_WM_ACTION_SHADE",
        "_NET_WM_ACTION_STICK",
        "_NET_WM_ACTION_MAXIMIZE_VERT",
        "_NET_WM_ACTION_MAXIMIZE_HORZ",
        "_NET_WM_ACTION_FULLSCREEN",
        "_NET_WM_ACTION_CHANGE_DESKTOP",
        "_NET_WM_ACTION_CLOSE",
        "_NET_WM_STRUT",
        "_NET_WM_STRUT_PARTIAL",
        "_NET_WM_ICON",
        "_NET_WM_ICON_GEOMETRY",
        "_NET_WM_PID",
        "_NET_WM_USER_TIME",
        "_NET_WM_FULL_PLACEMENT",
        "_NET_WM_CONTEXT_HELP",
        "_NET_WM_PING",
        "_NET_WM_SYNC_REQUEST",
        "WM_PROTOCOLS",
        "WM_DELETE_WINDOW",
        "WM_TAKE_FOCUS",
        "WM_STATE",
        "WM_CHANGE_STATE",
        "WM_TRANSIENT_FOR",
        "WM_HINTS",
        "WM_NORMAL_HINTS",
        "WM_CLIENT_LEADER",
        "WM_COLORMAP_WINDOWS",
        "WM_COLORMAP_NOTIFY",
        "WM_WINDOW_ROLE",
        "WM_CLIENT_MACHINE",
        "WM_COMMAND",
        "WM_LOCALE_NAME",
        "WM_ICON_SIZE",
        "WM_ICON_NAME",
        "_MOTIF_WM_HINTS",
        "_MOTIF_WM_INFO",
        "MANAGER",
        "TARGETS",
        "MULTIPLE",
        "TIMESTAMP",
        "INCR",
        "ATOM_PAIR",
    ];
    info!("PHASE 13: mixed openbox-style burst ({} InternAtom + extensions + grabs)",
          ATOMS.len());

    // Helper: pump replies until exactly one 32-byte frame arrives.
    // Counts EAGAIN cycles so we can confirm we're producing them.
    fn pump_one_reply(fd: libc::c_int, eagains: &mut u32) -> io::Result<[u8; 32]> {
        loop {
            let mut buf = [0u8; 32];
            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr() as *mut _,
                iov_len: buf.len(),
            };
            let mut msg: libc::msghdr = unsafe { core::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            let n = unsafe { libc::recvmsg(fd, &mut msg, 0) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EAGAIN)
                    || err.kind() == io::ErrorKind::WouldBlock
                {
                    *eagains += 1;
                    let mut pfd = libc::pollfd {
                        fd, events: libc::POLLIN, revents: 0,
                    };
                    let prc = unsafe { libc::ppoll(&mut pfd, 1,
                        &libc::timespec { tv_sec: 0, tv_nsec: 5_000_000 },
                        core::ptr::null()) };
                    if prc < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    continue;
                }
                return Err(err);
            }
            if n >= 32 {
                return Ok(buf);
            }
        }
    }

    // Helper: drain any pending non-reply frames (events / errors)
    // without blocking — exits as soon as recvmsg returns EAGAIN.
    fn drain_all_nonblock(fd: libc::c_int) -> u32 {
        let mut drained = 0;
        loop {
            let mut buf = [0u8; 32];
            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr() as *mut _,
                iov_len: buf.len(),
            };
            let mut msg: libc::msghdr = unsafe { core::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            let n = unsafe { libc::recvmsg(fd, &mut msg, 0) };
            if n <= 0 { return drained; }
            drained += 1;
        }
    }

    use std::time::{Duration, Instant};
    let mut out = Vec::with_capacity(128);
    let mut total_eagains = 0u32;
    let mut total_replies = 0u32;
    let mut sweeps = 0u32;
    // Loop the openbox-style burst for 30s so the test's xprop probe
    // (lands ~12-15s after kbox start) hits us mid-burst.  Each
    // sweep runs ATOMS + EXTS + grabs + GetProperty in libxcb-pump
    // style.
    let deadline = Instant::now() + Duration::from_secs(30);

    while Instant::now() < deadline {
    sweeps += 1;
    // 1. Burst the atoms with libxcb pump.
    for name in ATOMS {
        out.clear();
        let _seq = intern_atom(&mut out, conn, name, false);
        flush(&mut out, conn)?;
        let _r = pump_one_reply(fd, &mut total_eagains)?;
        total_replies += 1;
    }

    // 2. Burst QueryExtensions (every reply has the same shape).
    const EXTS: &[&str] = &[
        "BIG-REQUESTS", "XKEYBOARD", "MIT-SHM", "RANDR", "DAMAGE",
        "RENDER", "Composite", "GLX", "DRI3", "Present",
        "XInputExtension", "XFIXES", "DPMS", "SHAPE", "SYNC",
        "XC-MISC", "XINERAMA", "MIT-SCREEN-SAVER", "X-Resource",
        "XVideo",
    ];
    for ext in EXTS {
        out.clear();
        let _ = query_extension(&mut out, conn, ext);
        flush(&mut out, conn)?;
        let _r = pump_one_reply(fd, &mut total_eagains)?;
        total_replies += 1;
    }

    // 3. Burst many GrabKey requests (no reply, but each writev is
    // separate and triggers Xorg's input-grab path).  Cover
    // keycodes 8..120, AnyModifier.
    out.clear();
    for k in 8u8..120u8 {
        let _ = crate::req::grab_key(&mut out, conn, false,
                                     conn.info.root_xid, 0x8000, k, 1, 1);
        // Flush every 16 grabs to mimic libxcb's small-batch flushes.
        if k % 16 == 15 {
            flush(&mut out, conn)?;
            // Don't await replies — GrabKey has none.  But drain
            // any error frames Xorg may have queued.
            total_eagains += drain_all_nonblock(fd);
        }
    }
    flush(&mut out, conn)?;
    total_eagains += drain_all_nonblock(fd);

    // 4. A handful of GetProperty requests on root, each pumped.
    for _ in 0..16 {
        out.clear();
        let _ = crate::req::get_property(&mut out, conn,
                                         conn.info.root_xid, 1 /*XA_PRIMARY*/);
        flush(&mut out, conn)?;
        let _r = pump_one_reply(fd, &mut total_eagains)?;
        total_replies += 1;
    }

    } // while
    info!("PHASE 13 done: {} sweeps, {} replies, {} EAGAIN-pump cycles in 30s",
          sweeps, total_replies, total_eagains);

    // Restore blocking mode so the post-phase idle loop's read_frame
    // works as expected.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ─── Phase 12 ───────────────────────────────────────────────────────────
// Resource creation cascade + window ops: AllocNamedColor (asks
// the server's colormap for a colour by name), CreateGC (graphics
// context bound to root), CreatePixmap (off-screen drawable),
// MapWindow + ConfigureWindow.  This is the chunk of openbox's
// startup that touches the server's resource ID + colormap +
// drawable systems.
//
// Hypothesis: a resource-leak or refcount divergence shows up
// here under our SubstructureRedirect grab (Phase 1) — Xorg's
// internal MapRequest re-emit might be the trigger.

pub fn phase12_resource_cascade(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 12 entry: AllocNamedColor + CreateGC + CreatePixmap + MapWindow + ConfigureWindow");
    let mut out = Vec::with_capacity(256);

    // Default colormap is on the screen — read it from xConnSetup
    // (we don't store it explicitly; cmap=0 selects the default
    // for the screen but X11 actually requires the actual XID).
    // Easiest: request via root's default cmap via ChangeProperty?
    // No — AllocNamedColor takes a CMAP xid.  The default is the
    // screen's `default_colormap`, which is at offset 4 of the
    // screen block.  Phase 12 reads the screen block's default_cmap.
    //
    // Shortcut: skip color alloc for now; CreateGC + CreatePixmap +
    // MapWindow + ConfigureWindow exercise enough new opcodes.

    // Create a graphics context bound to root.
    let (s_gc, gc) = create_gc_default(&mut out, conn, conn.info.root_xid);
    // Create a small pixmap at root's depth.
    let (s_pix, pix) = create_pixmap(&mut out, conn, conn.info.root_depth,
                                     conn.info.root_xid, 32, 32);
    // Map the existing check_window from Phase 0.  It's InputOnly
    // (no pixels) so MapWindow is a no-op visually but exercises
    // the MapRequest → MapNotify path now that we hold
    // SubstructureRedirect on root.
    //
    // NOTE: We don't have direct access to check_window from this
    // function (become_wm returned it).  We can re-query via
    // QueryTree(root) or pass it through.  For simplicity: map a
    // *fresh* InputOnly window we create here.
    let (s_cw, child) = create_window_input_only(
        &mut out, conn, conn.info.root_xid, conn.info.root_visual);
    let s_map = map_window(&mut out, conn, child);

    // Configure: move + resize + raise.
    // Mask bits: 0x01=x, 0x02=y, 0x04=width, 0x08=height, 0x40=stack_mode.
    // stack_mode value 0=Above.
    let cfg_mask: u16 = 0x4F;
    let cfg_values = &[10u32, 20u32, 64u32, 64u32, 0u32];
    let s_cfg = configure_window(&mut out, conn, child, cfg_mask, cfg_values);
    let _ = (s_gc, s_pix, s_cw, s_map, s_cfg);
    flush(&mut out, conn)?;

    // None of these have replies; errors arrive async.  Drain a
    // few frames in case Xorg sent us MapRequest/ConfigureRequest
    // events (we hold SubstructureRedirect on root from phase 1,
    // so any child's map/config goes through our queue first).
    use std::os::unix::io::AsRawFd;
    let fd = conn.sock.as_raw_fd();
    let mut drained = 0u32;
    for _ in 0..20 {
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let rc = unsafe { libc::poll(&mut pfd, 1, 100) };
        if rc <= 0 || (pfd.revents & libc::POLLIN) == 0 { break; }
        match crate::reply::read_frame(conn) {
            Ok(_) => { drained += 1; }
            Err(_) => break,
        }
    }
    info!("PHASE 12: drained {} async frames", drained);

    // Cleanup.
    free_pixmap(&mut out, conn, pix);
    free_gc(&mut out, conn, gc);
    flush(&mut out, conn)?;

    info!("PHASE 12 done: resources cycled (gc=0x{:x} pix=0x{:x} child=0x{:x})",
          gc, pix, child);
    Ok(())
}

// ─── Phase 11 ───────────────────────────────────────────────────────────
// MIT-SHM attach.  Real openbox uses MIT-SHM via libxcb to push
// pixmap data faster than over the X11 socket.  The attach
// involves: shmget (create segment), shmat (attach to our address
// space), MitShmAttach (server-side handle), shmctl(IPC_RMID) to
// auto-cleanup on detach.
//
// Hypothesis: our SysV-shm + AF_UNIX-fd-passing combo has a
// regression that hangs Xorg's reply pump.  Tested mostly via
// unit tests; never exercised in a WM context with concurrent
// other-client traffic.

#[allow(unsafe_code)]
pub fn phase11_mit_shm(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 11 entry: MIT-SHM QueryVersion + Attach");
    let mut out = Vec::with_capacity(128);

    let s_qe = query_extension(&mut out, conn, "MIT-SHM");
    flush(&mut out, conn)?;
    let qe = await_reply(conn, s_qe)?;
    let qer = parse_query_extension_reply(&qe);
    let shm_major = match qer {
        Some(r) if r.present => r.major_opcode,
        _ => {
            info!("PHASE 11: MIT-SHM not present; skipping");
            return Ok(());
        }
    };
    info!("MIT-SHM major_opcode={}", shm_major);

    // Negotiate version.
    let s_qv = shm_query_version(&mut out, conn, shm_major);
    flush(&mut out, conn)?;
    let qv = await_reply(conn, s_qv)?;
    if let Frame::Reply { extra, .. } = &qv {
        info!("MitShmQueryVersion reply extra_bytes={}", extra.len());
    }

    // Create a 64 KiB SysV SHM segment.  IPC_PRIVATE=0,
    // IPC_CREAT=0o1000, mode 0o600.  Use libc directly.
    const SIZE: usize = 64 * 1024;
    let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, SIZE, 0o1000 | 0o600) };
    if shmid < 0 {
        let err = std::io::Error::last_os_error();
        err_!("shmget failed: {}", err);
        return Err(err);
    }
    info!("shmget → shmid={}", shmid);

    // Attach to our own address space (we don't actually need the
    // pointer; we just want the segment alive long enough for the
    // server to attach).
    let addr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) };
    if addr as isize == -1 {
        let err = std::io::Error::last_os_error();
        err_!("shmat failed: {}", err);
        unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()); }
        return Err(err);
    }
    info!("shmat → addr={:p}", addr);

    // Tell the server about it.  Allocate a fresh resource ID.
    let shmseg = conn.alloc_xid();
    let _s = shm_attach(&mut out, conn, shm_major, shmseg, shmid as u32, false);
    flush(&mut out, conn)?;

    // Mark the segment for deletion now — the kernel keeps it
    // alive until the last attacher detaches.  Clean shutdown
    // even if we crash later.
    unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()); }

    info!("PHASE 11 done: SHM attached as shmseg=0x{:x}", shmseg);
    Ok(())
}

// ─── Phase 10 ───────────────────────────────────────────────────────────
// XKB initialisation.  Real openbox does (at least): XkbUseExtension,
// XkbGetState, XkbGetControls, XkbGetNames(which=0xFFFFFFFF).  These
// use the XKB extension's major_opcode (returned by Phase 9's
// QueryExtension reply) and *minor* opcodes in the data byte.
//
// Hypothesis: extension routing in our Xorg-traffic path may
// mishandle the second-byte opcode discrimination, leading to
// silent drops or misroutes that hang the server's reply pipe.

pub fn phase10_xkb(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 10 entry: XkbUseExtension + XkbGetState + XkbGetControls + XkbGetNames");
    let mut out = Vec::with_capacity(256);

    // Look up the XKEYBOARD extension's major_opcode.  Phase 9 has
    // already done this once but didn't memoize; re-query.
    let s_qe = query_extension(&mut out, conn, "XKEYBOARD");
    flush(&mut out, conn)?;
    let qe = await_reply(conn, s_qe)?;
    let qer = parse_query_extension_reply(&qe);
    let xkb_major = match qer {
        Some(r) if r.present => r.major_opcode,
        _ => {
            info!("PHASE 10: XKEYBOARD not present; skipping");
            return Ok(());
        }
    };
    info!("XKEYBOARD major_opcode={}", xkb_major);

    // Negotiate XKB version and read state.
    let s_use   = xkb_use_extension(&mut out, conn, xkb_major, 1, 0);
    let s_state = xkb_get_state(&mut out, conn, xkb_major, 0x100);
    let s_ctrl  = xkb_get_controls(&mut out, conn, xkb_major, 0x100);
    let s_names = xkb_get_names(&mut out, conn, xkb_major, 0x100, 0xFFFFFFFF);
    flush(&mut out, conn)?;

    let f_use = await_reply(conn, s_use)?;
    if let Frame::Reply { header, .. } = &f_use {
        info!("XkbUseExtension supported={} server={}.{}",
              header[1] != 0,
              crate::wire::get_u16(header, 8),
              crate::wire::get_u16(header, 10));
    }
    let f_state = await_reply(conn, s_state)?;
    if let Frame::Reply { extra, .. } = &f_state {
        info!("XkbGetState reply extra_bytes={}", extra.len());
    }
    let f_ctrl = await_reply(conn, s_ctrl)?;
    if let Frame::Reply { extra, .. } = &f_ctrl {
        info!("XkbGetControls reply extra_bytes={}", extra.len());
    }
    let f_names = await_reply(conn, s_names)?;
    if let Frame::Reply { extra, .. } = &f_names {
        info!("XkbGetNames reply extra_bytes={}", extra.len());
    }
    info!("PHASE 10 done: XKB negotiated");
    Ok(())
}

// ─── Phase 9 ────────────────────────────────────────────────────────────
// Extension negotiation.  Real openbox queries every extension it
// might use; the responses include a major_opcode that openbox
// uses for subsequent requests.  If our QueryExtension reply
// returns wrong (present/major_opcode/first_event/first_error)
// values for any of these, openbox could end up sending garbled
// requests that hang Xorg.

const EXTENSIONS_TO_PROBE: &[&str] = &[
    "BIG-REQUESTS",
    "XKEYBOARD",
    "MIT-SHM",
    "RANDR",
    "DAMAGE",
    "RENDER",
    "Composite",
    "GLX",
    "DRI3",
    "Present",
    "XInputExtension",
    "XFIXES",
    "DPMS",
    "SHAPE",
    "SYNC",
    "XC-MISC",
];

pub fn phase9_query_extensions(conn: &mut X11Conn) -> io::Result<()> {
    info!("PHASE 9 entry: QueryExtension for {} extensions",
          EXTENSIONS_TO_PROBE.len());
    let mut out = Vec::with_capacity(512);
    let mut seqs = Vec::with_capacity(EXTENSIONS_TO_PROBE.len());
    for name in EXTENSIONS_TO_PROBE {
        let s = query_extension(&mut out, conn, name);
        seqs.push((*name, s));
    }
    flush(&mut out, conn)?;

    for (name, s) in &seqs {
        let f = await_reply(conn, *s)?;
        let r = parse_query_extension_reply(&f);
        match r {
            Some(r) if r.present => {
                info!("  ext {:?} present major_opcode={} first_event={} first_error={}",
                      name, r.major_opcode, r.first_event, r.first_error);
            }
            Some(_) => {
                info!("  ext {:?} not present", name);
            }
            None => {
                err_!("  ext {:?} reply parse failed", name);
            }
        }
    }
    info!("PHASE 9 done: extension negotiation complete");
    Ok(())
}

// ─── Phase 8 ────────────────────────────────────────────────────────────
// Aggressive async batching — fire 64 GetProperty requests in one
// flush, then drain 64 replies; repeat for 30s with no sleep.  This
// is much closer to libev's batched epoll_pwait behaviour than
// Phase 7's serialised round-trips.  If the hang is sensitive to
// in-flight request depth or to AF_UNIX rx-buffer fill rate, this
// is where it should surface.

pub fn phase8_async_storm(conn: &mut X11Conn) -> io::Result<()> {
    use std::time::{Duration, Instant};
    info!("PHASE 8 entry: async batched GetProperty storm (~30s, batch=64)");

    let mut out = Vec::with_capacity(2048);
    let s_intern = intern_atom(&mut out, conn, "_NET_SUPPORTING_WM_CHECK", false);
    flush(&mut out, conn)?;
    let f = await_reply(conn, s_intern)?;
    let probe_atom = parse_intern_atom_reply(&f).unwrap_or(0);
    if probe_atom == 0 {
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut total = 0u32;
    let mut batches = 0u32;
    while Instant::now() < deadline {
        let mut seqs = Vec::with_capacity(64);
        for _ in 0..64 {
            let s = crate::req::get_property(&mut out, conn, conn.info.root_xid, probe_atom);
            seqs.push(s);
        }
        flush(&mut out, conn)?;
        for s in seqs {
            let _ = await_reply(conn, s)?;
            total += 1;
        }
        batches += 1;
    }
    info!("PHASE 8 done: {} batches × ~64 reqs = {} total in 30s", batches, total);
    Ok(())
}

pub fn phase7_event_loop(conn: &mut X11Conn) -> io::Result<()> {
    use std::time::{Duration, Instant};
    info!("PHASE 7 entry: tight GetProperty loop (~30s) to mimic openbox/libev polling");

    // Pick a known root atom to spam GetProperty against — use
    // _NET_SUPPORTING_WM_CHECK from Phase 0.
    let mut out = Vec::with_capacity(64);
    let s_intern = intern_atom(&mut out, conn, "_NET_SUPPORTING_WM_CHECK", false);
    flush(&mut out, conn)?;
    let f = await_reply(conn, s_intern)?;
    let probe_atom = parse_intern_atom_reply(&f).unwrap_or(0);
    if probe_atom == 0 {
        info!("PHASE 7: failed to intern probe atom; aborting");
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut iters = 0u32;
    while Instant::now() < deadline {
        let s = crate::req::get_property(&mut out, conn, conn.info.root_xid, probe_atom);
        flush(&mut out, conn)?;
        let _ = await_reply(conn, s)?;
        iters += 1;
        // Sleep ~5ms between iterations — exactly the libev cadence.
        std::thread::sleep(Duration::from_millis(5));
    }
    info!("PHASE 7 done: {} GetProperty iterations across 30s", iters);
    Ok(())
}

fn expect_atom(frame: Frame, name: &str) -> io::Result<u32> {
    match &frame {
        Frame::Reply { .. } => {
            let atom = parse_intern_atom_reply(&frame)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData,
                                              "InternAtom reply parse failed"))?;
            if atom == 0 {
                err_!("InternAtom({:?}) returned None (atom=0)", name);
                return Err(io::Error::new(io::ErrorKind::Other,
                                          format!("atom {:?} not interned", name)));
            }
            Ok(atom)
        }
        Frame::Error { code, bad_value, .. } => {
            err_!("InternAtom({:?}) failed: code={} bad=0x{:x}", name, code, bad_value);
            Err(io::Error::new(io::ErrorKind::Other,
                              format!("InternAtom {:?} returned X11 error {}", name, code)))
        }
        Frame::Event { .. } => {
            // Shouldn't happen since await_reply filters events, but be safe.
            Err(io::Error::new(io::ErrorKind::Other, "unexpected event"))
        }
    }
}
