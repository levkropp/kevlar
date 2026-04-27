// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Linux kABI compatibility — module loader (milestone K1).
//!
//! This is the foundational primitive of the Kevlar-as-Linux-kernel-
//! replacement arc.  Loads ET_REL ELF objects ("`.ko` files") into
//! kernel memory, resolves their symbols against a kernel-exported
//! symbol table, applies relocations, and calls their entry function.
//!
//! K1 scope: minimal hello-world `.ko` calling a kernel-exported
//! `printk`.  Nothing of Linux's `struct module` / `.modinfo` /
//! `.gnu.linkonce.this_module` machinery is present yet (that's K2).
//!
//! Pinned target: Linux 7.0 (Ubuntu 26.04 LTS "Resolute Raccoon").

pub mod alloc;
pub mod arch;
pub mod completion;
pub mod elf;
pub mod exports;
pub mod loader;
pub mod modinfo;
pub mod printk;
pub mod reloc;
pub mod sched;
pub mod symbols;
pub mod wait;
pub mod work;

pub use loader::{load_module, LoadedModule};

/// Boot-time initialization for the kABI runtime: spawns the
/// workqueue worker kthread.  Call after the scheduler is up.
pub fn init() {
    work::init();
    log::info!("kabi: runtime initialized (workqueue spawned)");
}
