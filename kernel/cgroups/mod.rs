// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! cgroups v2 unified hierarchy.
pub mod cgroupfs;
pub mod pids_controller;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::once::Once;

use crate::process::PId;

/// Controller bitflags for cgroup.subtree_control.
pub const CTRL_CPU: u32 = 1;
pub const CTRL_MEMORY: u32 = 2;
pub const CTRL_PIDS: u32 = 4;
pub const CTRL_ALL: u32 = CTRL_CPU | CTRL_MEMORY | CTRL_PIDS;

/// Global root cgroup node.
pub static CGROUP_ROOT: Once<Arc<CgroupNode>> = Once::new();

/// A node in the cgroup v2 hierarchy.
pub struct CgroupNode {
    pub name: String,
    pub parent: Option<alloc::sync::Weak<CgroupNode>>,
    pub children: SpinLock<BTreeMap<String, Arc<CgroupNode>>>,
    /// PIDs belonging to this cgroup.
    pub member_pids: SpinLock<alloc::vec::Vec<PId>>,
    /// Controllers enabled for children (written via cgroup.subtree_control).
    pub subtree_control: AtomicU32,
    /// pids.max limit (-1 = unlimited).
    pub pids_max: AtomicI64,
    /// memory.max limit (-1 = unlimited, stub).
    pub memory_max: AtomicI64,
    /// cpu.max quota in microseconds (-1 = unlimited, stub).
    pub cpu_max_quota: AtomicI64,
    /// cpu.max period in microseconds (default 100000, stub).
    pub cpu_max_period: AtomicI64,
}

impl CgroupNode {
    pub fn new_root() -> Arc<CgroupNode> {
        Arc::new(CgroupNode {
            name: String::new(),
            parent: None,
            children: SpinLock::new(BTreeMap::new()),
            member_pids: SpinLock::new(alloc::vec::Vec::new()),
            subtree_control: AtomicU32::new(0),
            pids_max: AtomicI64::new(-1),
            memory_max: AtomicI64::new(-1),
            cpu_max_quota: AtomicI64::new(-1),
            cpu_max_period: AtomicI64::new(100_000),
        })
    }

    pub fn new_child(name: &str, parent: &Arc<CgroupNode>) -> Arc<CgroupNode> {
        Arc::new(CgroupNode {
            name: String::from(name),
            parent: Some(Arc::downgrade(parent)),
            children: SpinLock::new(BTreeMap::new()),
            member_pids: SpinLock::new(alloc::vec::Vec::new()),
            subtree_control: AtomicU32::new(0),
            pids_max: AtomicI64::new(-1),
            memory_max: AtomicI64::new(-1),
            cpu_max_quota: AtomicI64::new(-1),
            cpu_max_period: AtomicI64::new(100_000),
        })
    }

    /// Returns the path of this cgroup relative to the root (e.g. "/test.scope").
    pub fn path(&self) -> String {
        let mut parts = alloc::vec::Vec::new();
        let mut node: Option<Arc<CgroupNode>> = self.parent.as_ref().and_then(|w| w.upgrade());
        // Collect ancestors (we need our own name too).
        parts.push(self.name.clone());
        while let Some(n) = node {
            if !n.name.is_empty() {
                parts.push(n.name.clone());
            }
            node = n.parent.as_ref().and_then(|w| w.upgrade());
        }
        parts.reverse();
        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            return String::from("/");
        }
        let mut path = String::new();
        for part in &parts {
            if !part.is_empty() {
                path.push('/');
                path.push_str(part);
            }
        }
        if path.is_empty() {
            String::from("/")
        } else {
            path
        }
    }

    /// Controllers available to this cgroup (inherited from parent's subtree_control).
    pub fn available_controllers(&self) -> u32 {
        match &self.parent {
            Some(weak) => {
                if let Some(parent) = weak.upgrade() {
                    parent.subtree_control.load(Ordering::Relaxed)
                } else {
                    CTRL_ALL // root has all
                }
            }
            None => CTRL_ALL, // root cgroup has all controllers available
        }
    }

    /// Count total PIDs in this cgroup subtree (recursively).
    /// Collects children under lock then releases before recursing to avoid
    /// holding the children spinlock across recursive calls.
    pub fn count_pids_recursive(&self) -> usize {
        let count = self.member_pids.lock().len();
        let children: alloc::vec::Vec<Arc<CgroupNode>> =
            self.children.lock().values().cloned().collect();
        children.iter().fold(count, |acc, child| acc + child.count_pids_recursive())
    }
}

impl core::fmt::Debug for CgroupNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CgroupNode({})", self.path())
    }
}

pub fn init() {
    CGROUP_ROOT.init(|| CgroupNode::new_root());
}
