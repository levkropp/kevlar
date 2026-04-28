// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux input-subsystem stubs (K12).
//!
//! These satisfy the linker for input-class modules
//! (`virtio_input.ko`, future `evdev.ko` / `joydev.ko` etc.).
//! K12 modules don't get their probe() invoked, so the actual
//! input_register_device / input_event paths never fire — the
//! stubs just keep the linker happy.

use core::ffi::c_void;

use crate::ksym;

/// Linux's `struct input_dev` is ~1.5 KB.  Allocate slightly
/// over that so any direct field write inside the (future) probe
/// stays in-bounds.
const INPUT_DEV_SIZE: usize = 2048;

#[unsafe(no_mangle)]
pub extern "C" fn input_allocate_device() -> *mut c_void {
    crate::kabi::alloc::kzalloc(INPUT_DEV_SIZE, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn input_free_device(dev: *mut c_void) {
    crate::kabi::alloc::kfree(dev);
}

#[unsafe(no_mangle)]
pub extern "C" fn input_register_device(_dev: *mut c_void) -> i32 {
    // K24: hand off to Kevlar's existing evdev infrastructure.
    // /dev/input/event0..3 are statically registered at boot; the
    // EvdevFile resolves to the i-th entry of
    // virtio_input::registered_devices(), so a successful push here
    // makes /dev/input/event0 (or eventN for the Nth registration)
    // userspace-visible.
    let name = alloc::string::String::from("kabi-virtio-input");
    let arc_dev = virtio_input::register_kabi_input_device(name);
    let n = virtio_input::registered_devices().len();
    let dev_name = arc_dev.name.lock().clone();
    log::info!(
        "kabi: input_register_device: registered '{}' as \
         /dev/input/event{} (total devices: {})",
        dev_name,
        n - 1,
        n,
    );
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn input_unregister_device(_dev: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn input_event(
    _dev: *mut c_void,
    _t: u32,
    _c: u32,
    _v: i32,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn input_alloc_absinfo(_dev: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn input_set_abs_params(
    _dev: *mut c_void,
    _axis: u32,
    _min: i32,
    _max: i32,
    _fuzz: i32,
    _flat: i32,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn input_mt_init_slots(
    _dev: *mut c_void,
    _num_slots: u32,
    _flags: u32,
) -> i32 {
    0
}

ksym!(input_allocate_device);
ksym!(input_free_device);
ksym!(input_register_device);
ksym!(input_unregister_device);
ksym!(input_event);
ksym!(input_alloc_absinfo);
ksym!(input_set_abs_params);
ksym!(input_mt_init_slots);
