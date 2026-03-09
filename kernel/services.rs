// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Service registry for Ring 2 services.
//!
//! The kernel Core accesses Ring 2 services through this module rather than
//! through concrete types. In Phase 4, calls through these service handles
//! will be wrapped in `catch_unwind` for panic containment.
use alloc::sync::Arc;
use kevlar_utils::once::Once;

use crate::net::service::NetworkStackService;

static NETWORK_STACK: Once<Arc<dyn NetworkStackService>> = Once::new();

/// Register the network stack service. Called once during boot.
pub fn register_network_stack(service: Arc<dyn NetworkStackService>) {
    NETWORK_STACK.init(|| service);
}

/// Access the registered network stack service.
pub fn network_stack() -> &'static Arc<dyn NetworkStackService> {
    &*NETWORK_STACK
}
