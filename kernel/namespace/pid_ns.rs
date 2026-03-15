// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! PID namespace: process ID isolation.
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicI32, Ordering};
use crate::process::PId;
use kevlar_platform::spinlock::SpinLock;

pub struct PidNamespace {
    /// Parent namespace (None for root).
    parent: Option<Arc<PidNamespace>>,
    /// Next PID to allocate in this namespace.
    next_pid: AtomicI32,
    /// Map: namespace-local PID → global PID.
    local_to_global: SpinLock<BTreeMap<PId, PId>>,
    /// Map: global PID → namespace-local PID.
    global_to_local: SpinLock<BTreeMap<PId, PId>>,
}

impl PidNamespace {
    /// Create the root PID namespace.
    pub fn root() -> PidNamespace {
        PidNamespace {
            parent: None,
            next_pid: AtomicI32::new(1),
            local_to_global: SpinLock::new(BTreeMap::new()),
            global_to_local: SpinLock::new(BTreeMap::new()),
        }
    }

    /// Create a child PID namespace.
    pub fn new_child(parent: &Arc<PidNamespace>) -> PidNamespace {
        PidNamespace {
            parent: Some(parent.clone()),
            next_pid: AtomicI32::new(1),
            local_to_global: SpinLock::new(BTreeMap::new()),
            global_to_local: SpinLock::new(BTreeMap::new()),
        }
    }

    /// Whether this is the root (initial) PID namespace.
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }

    /// Allocate a namespace-local PID for a process with the given global PID.
    /// Returns the namespace-local PID.
    pub fn alloc_ns_pid(&self, global_pid: PId) -> PId {
        if self.is_root() {
            // Root namespace: ns PID == global PID.
            return global_pid;
        }
        let ns_pid = PId::new(self.next_pid.fetch_add(1, Ordering::SeqCst));
        self.local_to_global.lock().insert(ns_pid, global_pid);
        self.global_to_local.lock().insert(global_pid, ns_pid);
        ns_pid
    }

    /// Translate a global PID to a namespace-local PID.
    /// Returns None if the PID is not visible in this namespace.
    #[allow(dead_code)]
    pub fn global_to_local(&self, global_pid: PId) -> Option<PId> {
        if self.is_root() {
            return Some(global_pid);
        }
        self.global_to_local.lock().get(&global_pid).copied()
    }

    /// Remove a PID mapping (called when a process exits).
    #[allow(dead_code)]
    pub fn remove_pid(&self, global_pid: PId) {
        if self.is_root() {
            return;
        }
        if let Some(ns_pid) = self.global_to_local.lock().remove(&global_pid) {
            self.local_to_global.lock().remove(&ns_pid);
        }
    }
}
