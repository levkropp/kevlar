// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! User pointers. Re-exports from kevlar_vfs plus kernel-internal types.
pub use kevlar_vfs::user_buffer::*;

use crate::prelude::*;
use kevlar_platform::address::UserVAddr;

/// Parses a bitflags field given from the user. Returns `Result<T>`.
macro_rules! bitflags_from_user {
    ($st:tt, $input:expr) => {{
        let bits = $input;
        $st::from_bits(bits).ok_or_else(|| {
            warn_once!(
                concat!("unsupported bitflags for ", stringify!($st), ": {:x}"),
                bits
            );

            crate::result::Error::new(crate::result::Errno::ENOSYS)
        })
    }};
}

/// A user-provided NULL-terminated string.
///
/// It's a copy of the string (not a reference) since the user can modify the
/// buffer anytime to cause bad things in the kernel.
pub(super) struct UserCStr {
    string: String,
}

impl UserCStr {
    pub fn new(uaddr: UserVAddr, max_len: usize) -> Result<UserCStr> {
        let mut tmp = vec![0; max_len];
        let copied_len = uaddr.read_cstr(tmp.as_mut_slice())?;
        let string = core::str::from_utf8(&tmp[..copied_len])
            .map_err(|_| Error::new(Errno::EINVAL))?
            .to_owned();
        Ok(UserCStr { string })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.string.as_bytes()
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }
}
