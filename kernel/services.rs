// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Service registry for Ring 2 services.
//!
//! Under Fortress/Balanced profiles, services are accessed through trait objects
//! (`Arc<dyn NetworkStackService>`) to enable catch_unwind at ring boundaries.
//!
//! Under Performance/Ludicrous profiles, services are accessed through concrete
//! types (`Arc<SmoltcpNetworkStack>`) — the compiler monomorphizes and inlines
//! all service calls, eliminating vtable dispatch.
use alloc::sync::Arc;
use kevlar_utils::once::Once;

use crate::net::service::NetworkStackService;

// --- Fortress / Balanced: trait object dispatch ---
#[cfg(any(feature = "profile-fortress", feature = "profile-balanced"))]
mod inner {
    use super::*;

    static NETWORK_STACK: Once<Arc<dyn NetworkStackService>> = Once::new();

    pub fn register_network_stack(service: Arc<dyn NetworkStackService>) {
        NETWORK_STACK.init(|| service);
    }

    pub fn network_stack() -> &'static Arc<dyn NetworkStackService> {
        &*NETWORK_STACK
    }
}

// --- Performance / Ludicrous: concrete type dispatch ---
#[cfg(any(feature = "profile-performance", feature = "profile-ludicrous"))]
mod inner {
    use super::*;
    use crate::net::SmoltcpNetworkStack;

    static NETWORK_STACK: Once<Arc<SmoltcpNetworkStack>> = Once::new();

    pub fn register_network_stack(service: Arc<SmoltcpNetworkStack>) {
        NETWORK_STACK.init(|| service);
    }

    pub fn network_stack() -> &'static Arc<SmoltcpNetworkStack> {
        &*NETWORK_STACK
    }
}

pub use inner::{network_stack, register_network_stack};
