// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! DAC (Discretionary Access Control) permission checking.
//!
//! Root (euid=0) bypasses all checks (CAP_DAC_OVERRIDE equivalent).
//! Non-root processes are checked against owner/group/other permission bits.

use crate::result::{Errno, Error, Result};
use kevlar_vfs::stat::Stat;

/// Desired access mode bits, matching the access(2) `mode` argument.
pub const F_OK: u32 = 0;
pub const R_OK: u32 = 4;
pub const W_OK: u32 = 2;
pub const X_OK: u32 = 1;

/// Check whether a process with the given credentials has `want` access
/// to an inode described by `stat`.
///
/// `want` is a bitmask of R_OK/W_OK/X_OK. F_OK (0) = existence check only.
/// Root (euid=0) bypasses all permission checks.
pub fn check_access(stat: &Stat, euid: u32, egid: u32, want: u32) -> Result<()> {
    // F_OK: existence check — always passes if we got here.
    if want == F_OK {
        return Ok(());
    }

    // Root bypasses DAC.
    if euid == 0 {
        return Ok(());
    }

    let mode = stat.mode.as_u32();
    let perm = if stat.uid.as_u32() == euid {
        (mode >> 6) & 7 // owner bits
    } else if stat.gid.as_u32() == egid {
        (mode >> 3) & 7 // group bits
    } else {
        mode & 7 // other bits
    };

    if (want & R_OK) != 0 && (perm & 4) == 0 {
        return Err(Error::new(Errno::EACCES));
    }
    if (want & W_OK) != 0 && (perm & 2) == 0 {
        return Err(Error::new(Errno::EACCES));
    }
    if (want & X_OK) != 0 && (perm & 1) == 0 {
        return Err(Error::new(Errno::EACCES));
    }
    Ok(())
}
