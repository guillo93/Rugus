//! Rugus network — thin smoltcp wrapper for embedded targets.

#![no_std]
#![warn(missing_docs)]

pub mod tcp;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::Device;
use smoltcp::socket::dhcpv4::{Event as DhcpEvent, Socket as DhcpSocket};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

pub use tcp::{tcp_connect, Endpoint, TcpError, TcpIo};

/// Default MAC for Rugus F769 examples (locally administered).
pub const DEFAULT_MAC: [u8; 6] = [0x02, 0x00, 0x52, 0x55, 0x47, 0x01];

/// Default ephemeral local TCP port for client sockets.
pub const DEFAULT_TCP_LOCAL_PORT: u16 = 49152;

/// Static IPv4 configuration.
#[derive(Clone, Copy, Debug)]
pub struct StaticConfig {
    /// Host address.
    pub address: Ipv4Address,
    /// Subnet prefix length.
    pub prefix_len: u8,
    /// Default gateway (optional).
    pub gateway: Option<Ipv4Address>,
}

impl StaticConfig {
    /// Typical LAN static host: 192.168.0.50/24, gateway .1
    pub const fn home_lan() -> Self {
        Self {
            address: Ipv4Address::new(192, 168, 0, 50),
            prefix_len: 24,
            gateway: Some(Ipv4Address::new(192, 168, 0, 1)),
        }
    }
}

/// IPv4 stack: smoltcp [`Interface`] + optional DHCP socket.
///
/// Uses one stack lifetime for both the PHY device and socket storage so TCP
/// IO adapters can borrow fields disjointly.
pub struct NetStack<'stack, D: Device> {
    iface: Interface,
    device: &'stack mut D,
    sockets: SocketSet<'stack>,
    dhcp_handle: Option<SocketHandle>,
}

impl<'stack, D: Device> NetStack<'stack, D> {
    /// Create interface with static IPv4 (no DHCP).
    pub fn new_static(
        mac: [u8; 6],
        cfg: StaticConfig,
        device: &'stack mut D,
        storage: &'stack mut [SocketStorage<'stack>],
    ) -> Self {
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ethernet(EthernetAddress(mac))),
            device,
            Instant::ZERO,
        );
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(IpAddress::Ipv4(cfg.address), cfg.prefix_len))
                .ok();
        });
        if let Some(gw) = cfg.gateway {
            iface.routes_mut().add_default_ipv4_route(gw).ok();
        }
        Self {
            iface,
            device,
            sockets: SocketSet::new(storage),
            dhcp_handle: None,
        }
    }

    /// Create interface that acquires IPv4 via DHCPv4.
    pub fn new_dhcp(
        mac: [u8; 6],
        device: &'stack mut D,
        storage: &'stack mut [SocketStorage<'stack>],
    ) -> Self {
        let iface = Interface::new(
            Config::new(HardwareAddress::Ethernet(EthernetAddress(mac))),
            device,
            Instant::ZERO,
        );
        let mut sockets = SocketSet::new(storage);
        let dhcp_handle = sockets.add(DhcpSocket::new());
        Self {
            iface,
            device,
            sockets,
            dhcp_handle: Some(dhcp_handle),
        }
    }

    /// Poll the stack once.
    pub fn poll(&mut self, now: Instant) {
        self.iface.poll(now, self.device, &mut self.sockets);
    }

    /// smoltcp interface context (for TCP connect).
    pub fn context(&mut self) -> &mut smoltcp::iface::Context {
        self.iface.context()
    }

    /// Mutable socket set.
    pub fn sockets_mut(&mut self) -> &mut SocketSet<'stack> {
        &mut self.sockets
    }

    /// Mutable reference to the Ethernet device.
    pub fn device_mut(&mut self) -> &mut D {
        self.device
    }

    /// First assigned IPv4 address, if any.
    pub fn ipv4(&self) -> Option<Ipv4Address> {
        self.iface.ipv4_addr()
    }

    /// Process DHCP events; returns true when bound/renewed.
    pub fn poll_dhcp(&mut self) -> bool {
        let Some(handle) = self.dhcp_handle else {
            return false;
        };
        let socket = self.sockets.get_mut::<DhcpSocket>(handle);
        match socket.poll() {
            Some(DhcpEvent::Configured(config)) => {
                self.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    addrs.push(IpCidr::Ipv4(config.address)).ok();
                });
                if let Some(router) = config.router {
                    self.iface.routes_mut().add_default_ipv4_route(router).ok();
                }
                true
            }
            Some(DhcpEvent::Deconfigured) => {
                self.iface.update_ip_addrs(|addrs| addrs.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
                false
            }
            None => false,
        }
    }
}
