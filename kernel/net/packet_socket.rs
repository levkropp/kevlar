// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! AF_PACKET stub socket.
//!
//! Creates a valid file descriptor that accepts bind/setsockopt but never
//! delivers packets. Programs that probe AF_PACKET (dhcpcd, tcpdump)
//! can start without crashing. recvfrom returns EAGAIN in nonblock mode.
use core::fmt;

use alloc::sync::Arc;

use crate::{
    fs::{
        inode::{FileLike, PollStatus},
        opened_file::OpenOptions,
    },
    net::socket::SockAddr,
    result::{Errno, Result},
    user_buffer::{UserBuffer, UserBufferMut},
};

pub struct PacketSocket {
    socket_type: i32,
}

impl PacketSocket {
    pub fn new(socket_type: i32) -> Arc<PacketSocket> {
        Arc::new(PacketSocket { socket_type })
    }
}

impl fmt::Debug for PacketSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PacketSocket(type={})", self.socket_type)
    }
}

impl FileLike for PacketSocket {
    fn stat(&self) -> Result<kevlar_vfs::stat::Stat> {
        Ok(kevlar_vfs::stat::Stat::zeroed())
    }

    fn read(&self, _offset: usize, _buf: UserBufferMut<'_>, options: &OpenOptions) -> Result<usize> {
        if options.nonblock {
            Err(Errno::EAGAIN.into())
        } else {
            // Block until signal (no raw packets to deliver).
            super::SOCKET_WAIT_QUEUE.sleep_signalable_until(|| -> Result<Option<usize>> {
                Ok(None)
            })
        }
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        Ok(buf.len())
    }

    fn bind(&self, _sockaddr: SockAddr) -> Result<()> {
        Ok(())
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus::empty())
    }

    fn socket_type(&self) -> i32 {
        self.socket_type
    }

    fn recvfrom(
        &self,
        _buf: UserBufferMut<'_>,
        _flags: crate::net::RecvFromFlags,
        options: &OpenOptions,
    ) -> Result<(usize, SockAddr)> {
        if options.nonblock {
            Err(Errno::EAGAIN.into())
        } else {
            super::SOCKET_WAIT_QUEUE.sleep_signalable_until(|| -> Result<Option<(usize, SockAddr)>> {
                Ok(None)
            })
        }
    }

    fn getsockname(&self) -> Result<SockAddr> {
        Ok(SockAddr::In(crate::net::socket::SockAddrIn {
            family: 17, // AF_PACKET
            port: [0; 2],
            addr: [0; 4],
            zero: [0; 8],
        }))
    }
}
