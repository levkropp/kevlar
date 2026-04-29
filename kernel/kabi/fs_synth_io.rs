// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Read-folio implementation for kABI-synthesised struct file
//! handles.  K34 Day 2.
//!
//! Day 1 stub: log + return -ENOSYS so erofs's mount fails
//! cleanly with a recognisable error.  Day 2 fills in the real
//! initramfs-backed read.

use core::ffi::c_void;

/// `int read_folio(struct file *, struct folio *)` — Linux's
/// page-cache read entry point.  Erofs's fc_fill_super calls
/// this to read the on-disk superblock.
#[unsafe(no_mangle)]
pub extern "C" fn synth_read_folio(file: *mut c_void,
                                   folio: *mut c_void) -> i32 {
    log::warn!(
        "kabi: synth_read_folio(file={:p}, folio={:p}) — Day 2 stub",
        file, folio,
    );
    let lookup = super::fs_synth::lookup_synth_file(file as usize);
    match lookup {
        Some((path, size)) => {
            log::info!(
                "kabi: synth_read_folio: backing path={} size={}",
                path, size,
            );
            -38 // -ENOSYS — Day 2 will read folio_pgoff + page-aligned read here
        }
        None => {
            log::warn!("kabi: synth_read_folio: file {:p} not in synth table", file);
            -22 // -EINVAL
        }
    }
}
