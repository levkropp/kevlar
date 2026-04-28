// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! DRM GEM (Graphics Execution Manager) shim.
//!
//! `drm_gem_object_free` is a kref release callback that DRM modules
//! register on their gem objects.  drm_exec only references the
//! symbol; nothing invokes it at K14 load.  K15+ when a real DRM
//! driver actually creates and releases gem objects, this becomes
//! load-bearing.

use core::ffi::c_void;

use crate::ksym;

#[unsafe(no_mangle)]
pub extern "C" fn drm_gem_object_free(_kref: *mut c_void) {}

ksym!(drm_gem_object_free);
