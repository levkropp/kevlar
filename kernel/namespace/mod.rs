// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Linux namespace support: UTS, PID, mount.
pub mod uts;
pub mod pid_ns;
pub mod mnt;

use alloc::sync::Arc;
use crate::result::{Errno, Result};

pub use uts::UtsNamespace;
pub use pid_ns::PidNamespace;
pub use mnt::MountNamespace;

/// Clone flags for namespace creation.
pub const CLONE_NEWNS: usize   = 0x00020000;
pub const CLONE_NEWUTS: usize  = 0x04000000;
pub const CLONE_NEWPID: usize  = 0x20000000;
pub const CLONE_NEWNET: usize  = 0x40000000;

/// The set of namespaces a process belongs to.
#[derive(Clone)]
pub struct NamespaceSet {
    pub uts: Arc<UtsNamespace>,
    pub pid_ns: Arc<PidNamespace>,
    pub mnt: Arc<MountNamespace>,
}

impl NamespaceSet {
    /// Create the root (initial) namespace set.
    pub fn root() -> NamespaceSet {
        NamespaceSet {
            uts: Arc::new(UtsNamespace::new()),
            pid_ns: Arc::new(PidNamespace::root()),
            mnt: Arc::new(MountNamespace::new()),
        }
    }

    /// Clone this namespace set, creating new namespaces for any flags set.
    pub fn clone_with_flags(&self, flags: usize) -> Result<NamespaceSet> {
        if flags & CLONE_NEWNET != 0 {
            return Err(Errno::EINVAL.into());
        }

        let uts = if flags & CLONE_NEWUTS != 0 {
            Arc::new(self.uts.clone_ns())
        } else {
            self.uts.clone()
        };

        let pid_ns = if flags & CLONE_NEWPID != 0 {
            Arc::new(PidNamespace::new_child(&self.pid_ns))
        } else {
            self.pid_ns.clone()
        };

        let mnt = if flags & CLONE_NEWNS != 0 {
            Arc::new(self.mnt.clone_ns())
        } else {
            self.mnt.clone()
        };

        Ok(NamespaceSet { uts, pid_ns, mnt })
    }
}

static ROOT_NS: kevlar_utils::once::Once<NamespaceSet> = kevlar_utils::once::Once::new();

/// Direct access to root UTS namespace (avoids Arc clone in hot paths like uname).
pub static ROOT_UTS: kevlar_utils::once::Once<Arc<UtsNamespace>> = kevlar_utils::once::Once::new();

pub fn root_namespace_set() -> NamespaceSet {
    ROOT_NS.clone()
}

pub fn init() {
    let ns = NamespaceSet::root();
    ROOT_UTS.init(|| ns.uts.clone());
    ROOT_NS.init(|| ns);
}
