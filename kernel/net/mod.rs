// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::deferred_job::DeferredJob;
use crate::net::service::NetworkStackService as _;
use crate::{
    poll::POLL_WAIT_QUEUE, process::WaitQueue, timer::read_monotonic_clock, timer::MonotonicClock,
};
use alloc::boxed::Box;
use alloc::vec::Vec;
use atomic_refcell::AtomicRefCell;
use crossbeam::queue::ArrayQueue;
use kevlar_api::driver::net::EthernetDriver;
use kevlar_platform::bootinfo::BootInfo;
use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::once::Once;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::{self, EthernetAddress, EthernetFrame, HardwareAddress, IpCidr};

pub mod service;
pub mod socket;
mod icmp_socket;
mod tcp_socket;
mod udp_socket;
mod unix_socket;

pub use icmp_socket::*;
pub use socket::*;
pub use tcp_socket::*;
pub use udp_socket::*;
pub use unix_socket::*;

static PACKET_PROCESS_JOB: DeferredJob = DeferredJob::new("net_packet_process");
static RX_PACKET_QUEUE: Once<SpinLock<ArrayQueue<Vec<u8>>>> = Once::new();

pub fn receive_ethernet_frame(frame: &[u8]) {
    if RX_PACKET_QUEUE.lock().push(frame.to_vec()).is_err() {
        warn!("the rx packet queue is full; dropping an incoming packet");
    }

    PACKET_PROCESS_JOB.run_later(|| {
        crate::services::network_stack().process_packets();
    });
}

impl From<MonotonicClock> for Instant {
    fn from(value: MonotonicClock) -> Self {
        // FIXME: msecs could be larger than i64
        Instant::from_millis(value.msecs() as i64)
    }
}

static SOCKETS: Once<SpinLock<SocketSet<'static>>> = Once::new();
pub(crate) static INTERFACE: Once<SpinLock<Interface>> = Once::new();
static DHCP_HANDLE: Once<SocketHandle> = Once::new();
static DHCP_ENABLED: Once<bool> = Once::new();
static SOCKET_WAIT_QUEUE: Once<WaitQueue> = Once::new();

pub fn process_packets() {
    let mut sockets = SOCKETS.lock();
    let mut iface = INTERFACE.lock();

    let timestamp = read_monotonic_clock().into();

    if *DHCP_ENABLED {
        let dhcp_handle = *DHCP_HANDLE;
        let event = sockets
            .get_mut::<smoltcp::socket::dhcpv4::Socket>(dhcp_handle)
            .poll();
        if let Some(smoltcp::socket::dhcpv4::Event::Configured(config)) = event {
            let cidr = config.address;
            iface.update_ip_addrs(|addrs| {
                if let Some(addr) = addrs.iter_mut().next() {
                    *addr = IpCidr::Ipv4(cidr);
                }
            });
            info!("DHCP: got a IPv4 address: {}", cidr);

            if let Some(router) = config.router {
                iface
                    .routes_mut()
                    .add_default_ipv4_route(router)
                    .unwrap();
            }
        }
    }

    loop {
        match iface.poll(timestamp, &mut OurDevice, &mut sockets) {
            smoltcp::iface::PollResult::None => break,
            smoltcp::iface::PollResult::SocketStateChanged => {}
        }
    }

    SOCKET_WAIT_QUEUE.wake_all();
    POLL_WAIT_QUEUE.wake_all();
}

struct OurRxToken {
    buffer: Vec<u8>,
}

impl RxToken for OurRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&mut self.buffer)
    }
}

struct OurTxToken {}

impl TxToken for OurTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = vec![0; len];
        let return_value = f(&mut buffer);
        if EthernetFrame::new_checked(&mut buffer).is_ok() {
            use_ethernet_driver(|driver| driver.transmit(&buffer));
        }

        return_value
    }
}

struct OurDevice;

impl Device for OurDevice {
    type RxToken<'a> = OurRxToken;
    type TxToken<'a> = OurTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        RX_PACKET_QUEUE
            .lock()
            .pop()
            .map(|buffer| (OurRxToken { buffer }, OurTxToken {}))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(OurTxToken {})
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.medium = Medium::Ethernet;
        caps
    }
}

static ETHERNET_DRIVER: AtomicRefCell<Option<Box<dyn EthernetDriver>>> = AtomicRefCell::new(None);

pub fn register_ethernet_driver(driver: Box<dyn EthernetDriver>) {
    assert!(
        ETHERNET_DRIVER.borrow().is_none(),
        "multiple net drivers are not supported"
    );
    *ETHERNET_DRIVER.borrow_mut() = Some(driver);
}

pub fn use_ethernet_driver<F: FnOnce(&Box<dyn EthernetDriver>) -> R, R>(f: F) -> R {
    let driver = ETHERNET_DRIVER.borrow();
    f(driver.as_ref().expect("no ethernet drivers"))
}

#[derive(Debug)]
struct IPv4AddrParseError;

/// Parses an IPv4 address (e.g. "10.123.123.123").
fn parse_ipv4_addr(addr: &str) -> Result<wire::Ipv4Address, IPv4AddrParseError> {
    let mut iter = addr.splitn(4, '.');
    let mut octets = [0; 4];
    for octet in &mut octets {
        *octet = iter
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or(IPv4AddrParseError)?;
    }

    Ok(wire::Ipv4Address::from(octets))
}

/// Parses an IPv4 address with the prefix length (e.g. "10.123.123.123/24").
fn parse_ipv4_addr_with_prefix_len(
    addr: &str,
) -> Result<(wire::Ipv4Address, u8), IPv4AddrParseError> {
    let mut iter = addr.splitn(2, '/');
    let ip = parse_ipv4_addr(iter.next().unwrap())?;
    let prefix_len = iter
        .next()
        .ok_or(IPv4AddrParseError)?
        .parse()
        .map_err(|_| IPv4AddrParseError)?;

    Ok((ip, prefix_len))
}
pub fn init_and_start_dhcp_discover(bootinfo: &BootInfo) {
    if ETHERNET_DRIVER.borrow().is_none() {
        warn!("net: no ethernet driver, skipping network init");
        return;
    }

    let ip_addrs = match &bootinfo.ip4 {
        Some(ip4_str) => {
            let (ip4, prefix_len) = parse_ipv4_addr_with_prefix_len(ip4_str)
                .expect("bootinfo.ip4 should be formed as 10.0.0.1/24");
            info!("net: using a static IPv4 address: {}/{}", ip4, prefix_len);
            [IpCidr::new(ip4.into(), prefix_len)]
        }
        None => [IpCidr::new(wire::Ipv4Address::UNSPECIFIED.into(), 0)],
    };

    let mac_addr = use_ethernet_driver(|driver| driver.mac_addr());
    let ethernet_addr = EthernetAddress(mac_addr.as_array());
    let config = Config::new(HardwareAddress::Ethernet(ethernet_addr));
    let timestamp = read_monotonic_clock().into();
    let mut iface = Interface::new(config, &mut OurDevice, timestamp);

    iface.update_ip_addrs(|addrs| {
        for cidr in &ip_addrs {
            addrs.push(*cidr).unwrap();
        }
    });

    if let Some(gateway_ip4_str) = &bootinfo.gateway_ip4 {
        let gateway_ip4 = parse_ipv4_addr(gateway_ip4_str)
            .expect("bootinfo.gateway_ip4 should be formed as 10.0.0.1");
        info!("net: using a static gateway IPv4 address: {}", gateway_ip4);
        iface
            .routes_mut()
            .add_default_ipv4_route(gateway_ip4)
            .unwrap();
    };

    let mut sockets = SocketSet::new(vec![]);

    DHCP_ENABLED.init(|| bootinfo.dhcp_enabled);
    if *DHCP_ENABLED {
        let dhcp_socket = smoltcp::socket::dhcpv4::Socket::new();
        let dhcp_handle = sockets.add(dhcp_socket);
        DHCP_HANDLE.init(|| dhcp_handle);
    }
    RX_PACKET_QUEUE.init(|| SpinLock::new(ArrayQueue::new(128)));
    SOCKET_WAIT_QUEUE.init(WaitQueue::new);
    INTERFACE.init(|| SpinLock::new(iface));
    SOCKETS.init(|| SpinLock::new(sockets));

    process_packets();
}

/// The smoltcp-based network stack, implementing `NetworkStackService`.
pub struct SmoltcpNetworkStack;

impl service::NetworkStackService for SmoltcpNetworkStack {
    fn create_tcp_socket(&self) -> crate::result::Result<alloc::sync::Arc<dyn crate::fs::inode::FileLike>> {
        Ok(TcpSocket::new() as alloc::sync::Arc<dyn crate::fs::inode::FileLike>)
    }

    fn create_udp_socket(&self) -> crate::result::Result<alloc::sync::Arc<dyn crate::fs::inode::FileLike>> {
        Ok(UdpSocket::new() as alloc::sync::Arc<dyn crate::fs::inode::FileLike>)
    }

    fn create_unix_socket(&self) -> crate::result::Result<alloc::sync::Arc<dyn crate::fs::inode::FileLike>> {
        Ok(UnixSocket::new() as alloc::sync::Arc<dyn crate::fs::inode::FileLike>)
    }

    fn create_icmp_socket(&self) -> crate::result::Result<alloc::sync::Arc<dyn crate::fs::inode::FileLike>> {
        Ok(IcmpSocket::new() as alloc::sync::Arc<dyn crate::fs::inode::FileLike>)
    }

    fn process_packets(&self) {
        process_packets();
    }
}
