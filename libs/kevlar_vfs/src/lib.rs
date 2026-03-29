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

/// Set the VFS clock (called by the kernel timer subsystem).
pub fn set_vfs_clock(epoch_secs: u32) {
    VFS_CLOCK_SECS.store(epoch_secs, core::sync::atomic::Ordering::Relaxed);
}

/// Get current wall-clock seconds since epoch.
pub fn vfs_clock_secs() -> u32 {
    VFS_CLOCK_SECS.load(core::sync::atomic::Ordering::Relaxed)
}
