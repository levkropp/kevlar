// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! pids controller: enforces pids.max limits.
use super::CgroupNode;
use crate::result::{Errno, Result};
use core::sync::atomic::Ordering;

/// Check whether a fork is allowed under pids.max limits.
/// Walks from the cgroup up to the root, checking at each level.
/// Returns Err(EAGAIN) if any ancestor's pids.max would be exceeded.
pub fn check_fork_allowed(cgroup: &CgroupNode) -> Result<()> {
    check_node(cgroup)?;
    // Walk up to root checking each ancestor.
    let mut node = cgroup.parent.as_ref().and_then(|w| w.upgrade());
    while let Some(n) = node {
        check_node(&n)?;
        node = n.parent.as_ref().and_then(|w| w.upgrade());
    }
    Ok(())
}

fn check_node(node: &CgroupNode) -> Result<()> {
    let max = node.pids_max.load(Ordering::Relaxed);
    if max < 0 {
        return Ok(()); // unlimited
    }
    let current = node.count_pids_recursive();
    if current as i64 >= max {
        return Err(Errno::EAGAIN.into());
    }
    Ok(())
}
