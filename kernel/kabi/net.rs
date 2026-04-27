// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux network-device subsystem stubs (K11).
//!
//! Targeted at Ubuntu 26.04's `dummy.ko`: the stubs satisfy its 23
//! undefined symbol references so the linker resolves and
//! `init_module` returns 0.  Most are no-ops; only the RTNL +
//! link-ops registration paths see actual calls in the default-
//! numdummies=0 init flow.
//!
//! Real network functionality (registering /sys/class/net/dummy0,
//! routing packets) defers to a much later milestone — K11 is
//! "the linker is happy and init returns 0," nothing more.

use core::ffi::c_void;

use crate::ksym;

// ── rtnl (routing netlink) global lock ────────────────────────
// Linux's RTNL is a global mutex held during netdev modifications.
// Single-threaded module-init context: no-op.

#[unsafe(no_mangle)]
pub extern "C" fn rtnl_lock() {}

#[unsafe(no_mangle)]
pub extern "C" fn rtnl_unlock() {}

ksym!(rtnl_lock);
ksym!(rtnl_unlock);

// ── rtnl_link_ops registration ────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn rtnl_link_register(_ops: *mut c_void) -> i32 {
    log::info!("kabi: rtnl_link_register (stub)");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn rtnl_link_unregister(_ops: *mut c_void) {}

ksym!(rtnl_link_register);
ksym!(rtnl_link_unregister);

// ── netdev allocation / lifecycle ─────────────────────────────
//
// Linux's `struct net_device` is ~3 KB.  We allocate 4 KB so any
// direct field write (e.g. `dev->rtnl_link_ops` at offset 2328)
// stays inside our allocation.  The buffer is zero-init'd —
// callbacks reading fields see zeros, which is "no/none" for most
// fields and OK for K11's "load + return success" demo.

const NET_DEVICE_SIZE: usize = 4096;

#[unsafe(no_mangle)]
pub extern "C" fn alloc_netdev_mqs(
    _sizeof_priv: usize,
    _name: *const u8,
    _name_assign_type: u8,
    _setup: *mut c_void,
    _txqs: u32,
    _rxqs: u32,
) -> *mut c_void {
    crate::kabi::alloc::kzalloc(NET_DEVICE_SIZE, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn free_netdev(dev: *mut c_void) {
    crate::kabi::alloc::kfree(dev);
}

#[unsafe(no_mangle)]
pub extern "C" fn register_netdevice(_dev: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ether_setup(_dev: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn netif_carrier_on(_dev: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn netif_carrier_off(_dev: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn dev_addr_mod(
    _dev: *mut c_void,
    _offset: u32,
    _addr: *const u8,
    _len: usize,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn dev_lstats_read(
    _dev: *const c_void,
    _packets: *mut u64,
    _bytes: *mut u64,
) {
}

ksym!(alloc_netdev_mqs);
ksym!(free_netdev);
ksym!(register_netdevice);
ksym!(ether_setup);
ksym!(netif_carrier_on);
ksym!(netif_carrier_off);
ksym!(dev_addr_mod);
ksym!(dev_lstats_read);

// ── ethertool / ethernet helpers ──────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn eth_mac_addr(_dev: *mut c_void, _addr: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn eth_validate_addr(_dev: *mut c_void) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ethtool_op_get_ts_info(_dev: *mut c_void, _info: *mut c_void) -> i32 {
    0
}

ksym!(eth_mac_addr);
ksym!(eth_validate_addr);
ksym!(ethtool_op_get_ts_info);

// ── skb (socket buffer) lifecycle ─────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn consume_skb(_skb: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn skb_clone_tx_timestamp(_skb: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn skb_tstamp_tx(_skb: *mut c_void, _hwtstamps: *mut c_void) {}

ksym!(consume_skb);
ksym!(skb_clone_tx_timestamp);
ksym!(skb_tstamp_tx);
