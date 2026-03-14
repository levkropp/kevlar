// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Mount namespace: filesystem view isolation.
//!
//! For now this is a thin wrapper. Full mount namespace isolation
//! (per-namespace mount table) will be implemented with pivot_root in Phase 3.

pub struct MountNamespace {
    // Placeholder for future per-namespace mount table.
    _private: (),
}

impl MountNamespace {
    pub fn new() -> MountNamespace {
        MountNamespace { _private: () }
    }

    /// Clone this mount namespace.
    pub fn clone_ns(&self) -> MountNamespace {
        MountNamespace { _private: () }
    }
}
