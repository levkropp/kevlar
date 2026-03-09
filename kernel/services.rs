// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Service registry for Ring 2 services.
//!
//! Under Fortress/Balanced profiles, services are accessed through trait objects
//! (`Arc<dyn NetworkStackService>`) with `catch_unwind` at ring boundaries.
//! A panicking service returns `EIO` instead of crashing the kernel.
//!
//! Under Performance/Ludicrous profiles, services are accessed through concrete
//! types (`Arc<SmoltcpNetworkStack>`) — the compiler monomorphizes and inlines
//! all service calls, eliminating vtable dispatch. Panics crash the kernel.
use alloc::sync::Arc;
use kevlar_platform::capabilities::{self, Cap, NetAccess};
use kevlar_utils::once::Once;

use crate::net::service::NetworkStackService;

/// Call a service closure, catching panics under Fortress/Balanced profiles.
///
/// Under Fortress/Balanced: wraps the call in `catch_unwind`. If the service
/// panics, the panic is caught and converted to `Errno::EIO`.
///
/// Under Performance/Ludicrous: calls the closure directly with no overhead.
#[cfg(any(feature = "profile-fortress", feature = "profile-balanced"))]
pub fn call_service<R>(f: impl FnOnce() -> crate::result::Result<R>) -> crate::result::Result<R> {
    match unwinding::panic::catch_unwind(core::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<alloc::string::String>() {
                s.as_str()
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                s
            } else {
                "(unknown panic payload)"
            };
            warn!("service panicked, returning EIO: {}", msg);
            Err(crate::result::Errno::EIO.into())
        }
    }
}

#[cfg(any(feature = "profile-performance", feature = "profile-ludicrous"))]
#[inline(always)]
pub fn call_service<R>(f: impl FnOnce() -> crate::result::Result<R>) -> crate::result::Result<R> {
    f()
}

// --- Fortress / Balanced: trait object dispatch + capability tokens ---
#[cfg(any(feature = "profile-fortress", feature = "profile-balanced"))]
mod inner {
    use super::*;

    static NETWORK_STACK: Once<Arc<dyn NetworkStackService>> = Once::new();
    static NET_CAP: Once<Cap<NetAccess>> = Once::new();

    pub fn register_network_stack(service: Arc<dyn NetworkStackService>) {
        let cap = capabilities::mint::<NetAccess>();
        trace!("minted Cap<NetAccess> for network stack service");
        NET_CAP.init(|| cap);
        NETWORK_STACK.init(|| service);
    }

    pub fn network_stack() -> &'static Arc<dyn NetworkStackService> {
        // Fortress: validate capability token on each access.
        #[cfg(feature = "profile-fortress")]
        debug_assert!(
            capabilities::validate(&*NET_CAP),
            "network stack capability token is invalid"
        );
        &*NETWORK_STACK
    }
}

// --- Performance / Ludicrous: concrete type dispatch, no capabilities ---
#[cfg(any(feature = "profile-performance", feature = "profile-ludicrous"))]
mod inner {
    use super::*;
    use crate::net::SmoltcpNetworkStack;

    static NETWORK_STACK: Once<Arc<SmoltcpNetworkStack>> = Once::new();

    pub fn register_network_stack(service: Arc<SmoltcpNetworkStack>) {
        let _cap = capabilities::mint::<NetAccess>();
        NETWORK_STACK.init(|| service);
    }

    pub fn network_stack() -> &'static Arc<SmoltcpNetworkStack> {
        &*NETWORK_STACK
    }
}

pub use inner::{network_stack, register_network_stack};
