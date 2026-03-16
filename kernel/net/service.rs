// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Network stack service trait (Ring 2 boundary).
//!
//! This trait defines the interface between the Core (syscall dispatch) and
//! the network stack implementation (currently smoltcp). In Phase 4, calls
//! through this trait will be wrapped in `catch_unwind` for panic containment.
use alloc::sync::Arc;
use crate::fs::inode::FileLike;
use crate::result::Result;

/// A network stack service that can create sockets and process packets.
///
/// The Core calls these methods in response to syscalls and IRQs.
/// Implementations must be `Send + Sync` for use from any CPU context.
pub trait NetworkStackService: Send + Sync {
    /// Create a new TCP socket.
    fn create_tcp_socket(&self) -> Result<Arc<dyn FileLike>>;

    /// Create a new UDP socket.
    fn create_udp_socket(&self) -> Result<Arc<dyn FileLike>>;

    /// Create a new Unix domain socket.
    fn create_unix_socket(&self) -> Result<Arc<dyn FileLike>>;

    /// Create a new ICMP ping socket.
    fn create_icmp_socket(&self) -> Result<Arc<dyn FileLike>>;

    /// Process pending inbound/outbound packets. Called from deferred IRQ work.
    fn process_packets(&self);
}
