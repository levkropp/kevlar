// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! VFS trait definitions and shared types for the Kevlar kernel.
//!
//! This crate defines the interface types used across the kernel Core and
//! Ring 2 service crates (filesystems, network stack). It contains no
//! business logic — only trait definitions, type definitions, and error types.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod file_system;
pub mod inode;
pub mod path;
pub mod result;
pub mod socket_types;
pub mod stat;
pub mod user_buffer;

/// Global wall-clock seconds since epoch, updated by the kernel timer.
/// Filesystem services read this to stamp mtime/ctime on writes.
static VFS_CLOCK_SECS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Sub-second nanosecond component of the VFS clock (0..999_999_999).
static VFS_CLOCK_NSEC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Set the VFS clock (called by the kernel timer subsystem).
pub fn set_vfs_clock(epoch_secs: u32) {
    VFS_CLOCK_SECS.store(epoch_secs, core::sync::atomic::Ordering::Relaxed);
}

/// Set the VFS clock with nanosecond precision.
pub fn set_vfs_clock_ns(epoch_secs: u32, nsec: u32) {
    // Write nsec first, then secs, so readers see consistent or slightly stale nsec.
    VFS_CLOCK_NSEC.store(nsec, core::sync::atomic::Ordering::Relaxed);
    VFS_CLOCK_SECS.store(epoch_secs, core::sync::atomic::Ordering::Relaxed);
}

/// Get current wall-clock seconds since epoch.
pub fn vfs_clock_secs() -> u32 {
    VFS_CLOCK_SECS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Get current wall-clock as (seconds, nanoseconds).
pub fn vfs_clock_ts() -> (u32, u32) {
    let secs = VFS_CLOCK_SECS.load(core::sync::atomic::Ordering::Relaxed);
    let nsec = VFS_CLOCK_NSEC.load(core::sync::atomic::Ordering::Relaxed);
    (secs, nsec)
}
