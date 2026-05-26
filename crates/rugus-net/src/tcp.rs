//! TCP helpers — smoltcp socket IO adapter for `embedded-io`.

use core::fmt;

use embedded_io::{Error, ErrorKind, ErrorType, Read, Write};
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::phy::Device;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, Ipv4Address};

use crate::NetStack;

/// Remote endpoint (IPv4 + port).
#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    /// Server IPv4 address.
    pub addr: Ipv4Address,
    /// TCP port.
    pub port: u16,
}

impl Endpoint {
    /// Default LAN HTTPS test server for Rugus G4 examples.
    pub const fn lan_https_server() -> Self {
        Self {
            addr: Ipv4Address::new(192, 168, 0, 112),
            port: 8443,
        }
    }
}

/// TCP connect / IO error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpError {
    /// Timed out waiting for TCP state.
    Timeout,
    /// Socket closed before operation completed.
    Closed,
    /// smoltcp rejected the operation.
    InvalidState,
    /// Underlying stack error placeholder.
    WouldBlock,
}

impl core::error::Error for TcpError {}

impl Error for TcpError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Timeout | Self::WouldBlock => ErrorKind::TimedOut,
            Self::Closed => ErrorKind::ConnectionReset,
            Self::InvalidState => ErrorKind::InvalidInput,
        }
    }
}

impl fmt::Display for TcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("tcp timeout"),
            Self::Closed => f.write_str("tcp closed"),
            Self::InvalidState => f.write_str("tcp invalid state"),
            Self::WouldBlock => f.write_str("tcp would block"),
        }
    }
}

/// Blocking TCP connect: polls the stack until established or timeout.
pub fn tcp_connect<D: Device>(
    stack: &mut NetStack<'_, D>,
    handle: SocketHandle,
    remote: Endpoint,
    local_port: u16,
    now_ms: fn() -> u64,
    timeout_ms: u64,
) -> Result<(), TcpError> {
    {
        let NetStack { iface, sockets, .. } = stack;
        let cx = iface.context();
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        socket
            .connect(cx, (IpAddress::Ipv4(remote.addr), remote.port), local_port)
            .map_err(|_| TcpError::InvalidState)?;
    }

    let start = now_ms();
    loop {
        stack.poll(Instant::from_millis(now_ms() as i64));
        let socket = stack.sockets_mut().get_mut::<tcp::Socket>(handle);
        if socket.state() == tcp::State::Established {
            return Ok(());
        }
        if !socket.is_open() {
            return Err(TcpError::Closed);
        }
        if now_ms().saturating_sub(start) >= timeout_ms {
            return Err(TcpError::Timeout);
        }
    }
}

/// `embedded-io` adapter over a smoltcp TCP socket (blocking poll loop).
pub struct TcpIo<'stack, D: Device> {
    iface: &'stack mut Interface,
    device: &'stack mut D,
    sockets: &'stack mut SocketSet<'stack>,
    handle: SocketHandle,
    now_ms: fn() -> u64,
}

impl<'stack, D: Device> TcpIo<'stack, D> {
    /// Wrap a connected TCP socket handle (borrows stack fields disjointly).
    pub fn new(
        stack: &'stack mut NetStack<'stack, D>,
        handle: SocketHandle,
        now_ms: fn() -> u64,
    ) -> Self {
        let NetStack {
            iface,
            device,
            sockets,
            ..
        } = stack;
        Self {
            iface,
            device,
            sockets,
            handle,
            now_ms,
        }
    }

    fn poll_once(&mut self) {
        self.iface.poll(
            Instant::from_millis((self.now_ms)() as i64),
            self.device,
            self.sockets,
        );
    }
}

impl<'stack, D: Device> ErrorType for TcpIo<'stack, D> {
    type Error = TcpError;
}

impl<'stack, D: Device> Read for TcpIo<'stack, D> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            self.poll_once();
            let socket = self.sockets.get_mut::<tcp::Socket>(self.handle);
            if socket.can_recv() {
                return socket.recv_slice(buf).map_err(|e| match e {
                    tcp::RecvError::Finished => TcpError::Closed,
                    tcp::RecvError::InvalidState => TcpError::InvalidState,
                });
            }
            if !socket.is_open() {
                return Err(TcpError::Closed);
            }
        }
    }
}

impl<'stack, D: Device> Write for TcpIo<'stack, D> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            self.poll_once();
            let socket = self.sockets.get_mut::<tcp::Socket>(self.handle);
            if socket.can_send() {
                return socket.send_slice(buf).map_err(|_| TcpError::InvalidState);
            }
            if !socket.is_open() {
                return Err(TcpError::Closed);
            }
        }
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
