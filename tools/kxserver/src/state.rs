// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Server-wide shared state: atoms, resources, root window, framebuffer.
// Kept in its own file so handlers can borrow `&mut ServerState`
// disjointly from `&mut Client` (the client list lives on `Server`,
// not here).

use crate::atom::AtomTable;
use crate::fb::Framebuffer;
use crate::input::InputState;
use crate::resources::{Resource, ResourceMap};
use crate::setup;
use crate::window::Window;

pub struct ServerState {
    pub atoms:     AtomTable,
    pub resources: ResourceMap,
    pub fb:        Framebuffer,
    pub input:     InputState,
    /// Events destined for clients OTHER than the one currently being
    /// dispatched.  Handlers push here via `queue_event_to_client`;
    /// `server::poll_once` drains the list after each client's pump
    /// and routes to the matching clients.  Same-client events go
    /// straight onto `Client.event_queue`.
    pub pending_events: Vec<PendingEvent>,
    /// Selection owners keyed by selection atom.  Set by
    /// `SetSelectionOwner`, read by `GetSelectionOwner`.
    pub selection_owners: Vec<SelectionOwner>,
    /// SubstructureRedirect listener per window.  At most one client
    /// may hold the redirect on a given parent (X11 rule); we record
    /// the (window, client) pair so the pre-dispatch check can find
    /// the owner in O(N) without walking every window's listener
    /// list.  Empty list == no redirects currently held.
    pub redirect_owners: Vec<RedirectOwner>,
    /// Explicit "fire now" signal for `--inject` synthetic input.
    /// Flipped on by `handle_no_operation` (opcode 127) so tests can
    /// use `NoOperation` as an unambiguous "finished setup" marker.
    pub inject_armed: bool,
}

#[derive(Debug, Clone)]
pub struct PendingEvent {
    pub target_client: u32,
    pub ev: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct SelectionOwner {
    pub selection: u32,   // atom
    pub owner:     u32,   // window id (0 = None)
    pub client:    u32,
    pub time:      u32,
}

#[derive(Debug, Clone, Copy)]
pub struct RedirectOwner {
    pub parent: u32,
    pub client: u32,
}

impl ServerState {
    pub fn new() -> Self {
        let fb = Framebuffer::open(setup::SCREEN_WIDTH, setup::SCREEN_HEIGHT);
        let mut resources = ResourceMap::new();
        // Create the root window (server-owned, owner id = 0).
        let root = Window::new_root(
            setup::ROOT_WINDOW_ID,
            setup::SCREEN_WIDTH,
            setup::SCREEN_HEIGHT,
            setup::ROOT_VISUAL_ID,
        );
        resources.insert(setup::ROOT_WINDOW_ID, 0, Resource::Window(root));
        ServerState {
            atoms: AtomTable::new(),
            resources,
            fb,
            input: InputState::new(),
            pending_events: Vec::new(),
            selection_owners: Vec::new(),
            redirect_owners: Vec::new(),
            inject_armed: false,
        }
    }
}
