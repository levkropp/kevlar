// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::deferred_job::DeferredJob;
use crate::net::service::NetworkStackService as _;
use crate::{
    poll::POLL_WAIT_QUEUE, process::WaitQueue, timer::read_monotonic_clock, timer::MonotonicClock,
};
use alloc::boxed::Box;
use alloc::vec::Vec;
use atomic_refcell::AtomicRefCell;
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
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
pub mod netlink;
pub(crate) mod packet_socket;
mod tcp_socket;
mod udp_socket;
pub(crate) mod unix_socket;

pub use icmp_socket::*;
pub use socket::*;
pub use tcp_socket::*;
pub use udp_socket::*;
pub use unix_socket::*;

static PACKET_PROCESS_JOB: DeferredJob = DeferredJob::new("net_packet_process");
static RX_PACKET_QUEUE: Once<SpinLock<ArrayQueue<Vec<u8>>>> = Once::new();

pub fn receive_ethernet_frame(frame: &[u8]) {
    #[cfg(feature = "ktrace-net")]
    crate::debug::ktrace::trace(crate::debug::ktrace::event::NET_RX_PACKET,
        frame.len() as u32, 0, 0, 0, 0);

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

static NET_INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Poll the network stack from the LAPIC timer. Safe to call before init.
pub fn poll_if_ready() {
    if NET_INITIALIZED.load(core::sync::atomic::Ordering::Relaxed) {
        process_packets();
    }
}

/// True once `init_net` has populated SOCKETS / INTERFACE.  Used by
/// `sys_socket` to refuse AF_INET creation (would otherwise panic on
/// `SOCKETS.lock()`) on hosts without an ethernet driver — e.g. the
/// HVF/QEMU benchmark setup.
pub fn is_initialized() -> bool {
    NET_INITIALIZED.load(core::sync::atomic::Ordering::Relaxed)
}

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
            let oct = cidr.address().octets();
            set_own_ip(oct[0], oct[1], oct[2], oct[3]);

            if let Some(router) = config.router {
                iface
                    .routes_mut()
                    .add_default_ipv4_route(router)
                    .unwrap();
            }
        }
    }

    // Loop until no more work: iface.poll processes RX frames and generates
    // TX responses. For loopback, TX frames go back into RX queue, requiring
    // another poll round. Keep going until both poll returns None AND the
    // RX queue is empty (loopback frames fully drained).
    loop {
        match iface.poll(timestamp, &mut OurDevice, &mut sockets) {
            smoltcp::iface::PollResult::None => {
                // Check if loopback injected new frames that need processing.
                if RX_PACKET_QUEUE.lock().is_empty() {
                    break;
                }
            }
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

/// Set to `true` by OurTxToken::consume when an ARP request is transmitted.
/// Cleared before process_packets() by callers that need to detect whether
/// ARP was triggered (e.g. UDP sendto, to wait for the reply before the next
/// sendto can overwrite smoltcp's single-slot ARP pending cache).
pub(crate) static ARP_SENT: AtomicBool = AtomicBool::new(false);

impl TxToken for OurTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = vec![0; len];
        let return_value = f(&mut buffer);
        if EthernetFrame::new_checked(&mut buffer).is_ok() {
            #[cfg(feature = "ktrace-net")]
            crate::debug::ktrace::trace(crate::debug::ktrace::event::NET_TX_PACKET,
                buffer.len() as u32, 0, 0, 0, 0);

            // Detect ARP frames (EtherType 0x0806) so callers can wait for
            // the reply before sending another packet to the same destination.
            if buffer.len() >= 14 && buffer[12] == 0x08 && buffer[13] == 0x06 {
                ARP_SENT.store(true, AtomicOrdering::Relaxed);
            }

            // Loopback: if the destination IP is 127.0.0.0/8 or the interface's
            // own address, inject the frame back into the RX queue instead of
            // sending it out the wire. This enables same-host TCP connections.
            let is_loopback_ipv4 = buffer.len() >= 34
                && buffer[12] == 0x08 && buffer[13] == 0x00
                && (buffer[30] == 127 || is_own_ip(&buffer[30..34]));
            let is_loopback_arp = buffer.len() >= 42
                && buffer[12] == 0x08 && buffer[13] == 0x06
                && (buffer[38] == 127 || is_own_ip(&buffer[38..42]));

            if is_loopback_ipv4 || is_loopback_arp {
                // Swap src/dst MAC so smoltcp accepts the looped-back frame.
                let mut src_mac = [0u8; 6];
                src_mac.copy_from_slice(&buffer[6..12]);
                buffer.copy_within(0..6, 6);  // dst → src
                buffer[0..6].copy_from_slice(&src_mac);  // old src → dst

                if is_loopback_arp {
                    // Convert ARP request (opcode=1) to ARP reply (opcode=2)
                    // so smoltcp learns the MAC for the loopback address.
                    // Use a fake locally-administered MAC (02:00:00:7f:00:01)
                    // because smoltcp ignores ARP replies from its own MAC.
                    if buffer.len() >= 42 && buffer[21] == 1 {
                        buffer[21] = 2; // opcode = reply
                        let fake_mac: [u8; 6] = [0x02, 0x00, 0x00, 0x7f, 0x00, 0x01];
                        let target_ip: [u8; 4] = buffer[38..42].try_into().unwrap_or([0; 4]);
                        // Move original sender to target position
                        buffer.copy_within(22..32, 32);
                        // Set sender = fake loopback MAC + target IP
                        buffer[22..28].copy_from_slice(&fake_mac);
                        buffer[28..32].copy_from_slice(&target_ip);
                        // Also fix the Ethernet source MAC to the fake MAC
                        buffer[6..12].copy_from_slice(&fake_mac);
                    }
                }

                let _ = RX_PACKET_QUEUE.lock().push(buffer);
            } else {
                use_ethernet_driver(|driver| driver.transmit(&buffer));
            }
        }

        return_value
    }
}

/// Cached own IPv4 address bytes (set when DHCP or static IP is configured).
static OWN_IPV4: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Check if an IP address (4 bytes) matches the interface's configured address.
fn is_own_ip(ip: &[u8]) -> bool {
    let own = OWN_IPV4.load(core::sync::atomic::Ordering::Relaxed);
    if own == 0 { return false; }
    let own_bytes = own.to_be_bytes();
    ip == own_bytes
}

/// Update the cached own IP address.
pub(crate) fn set_own_ip(a: u8, b: u8, c: u8, d: u8) {
    let v = u32::from_be_bytes([a, b, c, d]);
    OWN_IPV4.store(v, core::sync::atomic::Ordering::Relaxed);
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
            let oct = ip4.octets();
            set_own_ip(oct[0], oct[1], oct[2], oct[3]);
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
        // Add loopback address after the real IP so ipv4_addr() returns the real one.
        addrs.push(IpCidr::new(wire::Ipv4Address::new(127, 0, 0, 1).into(), 8)).unwrap();
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
    NET_INITIALIZED.store(true, core::sync::atomic::Ordering::Relaxed);

    process_packets();
}

/// Format /proc/net/tcp — enumerate all TCP sockets.
pub fn format_proc_net_tcp() -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let _ = writeln!(s, "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode");

    let sockets = SOCKETS.lock();
    let mut sl = 0u32;
    for (_handle, socket) in sockets.iter() {
        if let smoltcp::socket::Socket::Tcp(tcp) = socket {
            let state: u8 = match tcp.state() {
                smoltcp::socket::tcp::State::Closed => 7,
                smoltcp::socket::tcp::State::Listen => 10,
                smoltcp::socket::tcp::State::SynSent => 2,
                smoltcp::socket::tcp::State::SynReceived => 3,
                smoltcp::socket::tcp::State::Established => 1,
                smoltcp::socket::tcp::State::FinWait1 => 4,
                smoltcp::socket::tcp::State::FinWait2 => 5,
                smoltcp::socket::tcp::State::CloseWait => 8,
                smoltcp::socket::tcp::State::Closing => 11,
                smoltcp::socket::tcp::State::LastAck => 9,
                smoltcp::socket::tcp::State::TimeWait => 6,
            };
            let local_str = match tcp.local_endpoint() {
                Some(ep) => ip_endpoint_to_hex(&ep),
                None => {
                    // For listening sockets, local_endpoint() returns None.
                    // Use listen_endpoint() to get the bound port.
                    let lep = tcp.listen_endpoint();
                    listen_endpoint_to_hex(lep.addr, lep.port)
                }
            };
            let remote_str = match tcp.remote_endpoint() {
                Some(ep) => ip_endpoint_to_hex(&ep),
                None => alloc::format!("00000000:0000"),
            };
            let _ = writeln!(s, "{:4}: {} {} {:02X} 00000000:00000000 00:00000000 00000000     0        0 0",
                sl, local_str, remote_str, state);
            sl += 1;
        }
    }
    s
}

/// Format /proc/net/udp — enumerate all UDP sockets.
pub fn format_proc_net_udp() -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let _ = writeln!(s, "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode");

    let sockets = SOCKETS.lock();
    let mut sl = 0u32;
    for (_handle, socket) in sockets.iter() {
        if let smoltcp::socket::Socket::Udp(udp) = socket {
            let ep = udp.endpoint();
            let local_str = listen_endpoint_to_hex(ep.addr, ep.port);
            let remote_str = "00000000:0000";
            let state: u8 = if udp.is_open() { 7 } else { 10 };
            let _ = writeln!(s, "{:4}: {} {} {:02X} 00000000:00000000 00:00000000 00000000     0        0 0",
                sl, local_str, remote_str, state);
            sl += 1;
        }
    }
    s
}

/// Format an IP address + port to hex "AABBCCDD:PORT".
fn ip_addr_to_hex(addr: &wire::IpAddress, port: u16) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    match addr {
        wire::IpAddress::Ipv4(v4) => {
            let b = v4.octets();
            let _ = write!(s, "{:02X}{:02X}{:02X}{:02X}:{:04X}", b[0], b[1], b[2], b[3], port);
        }
    }
    s
}

/// Format an IpEndpoint to hex.
fn ip_endpoint_to_hex(ep: &wire::IpEndpoint) -> alloc::string::String {
    ip_addr_to_hex(&ep.addr, ep.port)
}

/// Format a listen endpoint (addr is optional) to hex.
fn listen_endpoint_to_hex(addr: Option<wire::IpAddress>, port: u16) -> alloc::string::String {
    match addr {
        Some(a) => ip_addr_to_hex(&a, port),
        None => alloc::format!("00000000:{:04X}", port),
    }
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
