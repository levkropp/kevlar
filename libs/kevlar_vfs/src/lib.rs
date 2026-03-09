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
