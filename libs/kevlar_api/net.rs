// SPDX-License-Identifier: MIT OR Apache-2.0
//! Network APIs.
use crate::kernel_ops::kernel_ops;

pub fn receive_ethernet_frame(pkt: &[u8]) {
    kernel_ops().receive_etherframe_packet(pkt);
}
