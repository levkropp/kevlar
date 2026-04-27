// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! `.modinfo` section parser.
//!
//! Linux modules embed metadata via `MODULE_LICENSE` / `MODULE_AUTHOR`
//! / `MODULE_DESCRIPTION` etc. macros, which expand to
//! `__MODULE_INFO()` calls.  Each emits a NUL-terminated `key=value`
//! string into the `.modinfo` section.  Multiple entries are packed
//! back-to-back, delimited only by the terminating NUL of the
//! previous string.
//!
//! K2 parses this section to log module metadata at load time.  K3+
//! will use `depends=` to drive recursive module load and `vermagic=`
//! to gate ABI compatibility.

use alloc::string::String;

#[derive(Default)]
pub struct ModInfo {
    pub license: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub depends: Option<String>,
    pub srcversion: Option<String>,
    pub vermagic: Option<String>,
}

/// Parse `.modinfo` bytes (NUL-separated `key=value` packed strings).
pub fn parse(bytes: &[u8]) -> ModInfo {
    let mut info = ModInfo::default();
    let mut i = 0;
    while i < bytes.len() {
        // Skip any leading NUL padding.
        while i < bytes.len() && bytes[i] == 0 {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Find end of this NUL-terminated string.
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        let entry = &bytes[start..i];
        // Split on the first '='.
        if let Some(eq) = entry.iter().position(|&b| b == b'=') {
            let key = &entry[..eq];
            let val = &entry[eq + 1..];
            let val_str = match core::str::from_utf8(val) {
                Ok(s) => s,
                Err(_) => continue,
            };
            match key {
                b"license" => info.license = Some(String::from(val_str)),
                b"author" => info.author = Some(String::from(val_str)),
                b"description" => info.description = Some(String::from(val_str)),
                b"version" => info.version = Some(String::from(val_str)),
                b"depends" => info.depends = Some(String::from(val_str)),
                b"srcversion" => info.srcversion = Some(String::from(val_str)),
                b"vermagic" => info.vermagic = Some(String::from(val_str)),
                _ => {}
            }
        }
    }
    info
}
