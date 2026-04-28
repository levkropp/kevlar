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
pub mod bitops;
pub mod bus;
pub mod cdev;
pub mod completion;
pub mod cpufeature;
pub mod device;
pub mod dma;
pub mod dma_resv;
pub mod drm;
pub mod drm_client;
pub mod drm_fb_helper;
pub mod drm_format;
pub mod drm_gem;
pub mod elf;
pub mod exports;
pub mod fb;
pub mod fb_raster;
pub mod fops;
pub mod input;
pub mod io;
pub mod kobject;
pub mod kref;
pub mod list;
pub mod loader;
pub mod mem;
pub mod modinfo;
pub mod modparam;
pub mod module_ref;
pub mod mutex;
pub mod net;
pub mod platform;
pub mod printk;
pub mod printk_fmt;
pub mod random;
pub mod rbtree;
pub mod refcount;
pub mod reloc;
pub mod scatterlist;
pub mod sched;
pub mod slab;
pub mod spinlock;
pub mod stack;
pub mod symbols;
pub mod ttm;
pub mod ubsan;
pub mod usercopy;
pub mod virtio;
pub mod wait;
pub mod work;
pub mod ww_mutex;

pub use loader::{load_module, LoadedModule};

/// Boot-time initialization for the kABI runtime: spawns the
/// workqueue worker kthread + initializes the platform bus.  Call
/// after the scheduler is up.
pub fn init() {
    work::init();
    platform::init();
    log::info!("kabi: runtime initialized (workqueue + platform bus)");
}
