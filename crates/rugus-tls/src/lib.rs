//! Rugus TLS — thin wrapper over [`embedded_tls::blocking`] for LAN HTTPS clients.
//!
//! Certificate verification is **disabled** by default via [`UnsecureProvider`] +
//! [`NoVerify`], suitable for self-signed lab servers. Production firmware should
//! pin server certificates (future: `rustpki` feature on `embedded-tls`).
//!
//! # Usage
//!
//! ```ignore
//! use embedded_io::{Read, Write};
//! use rugus_crypto::SoftwareRng;
//! use rugus_tls::{Aes128GcmSha256, TlsClient, TlsConfig};
//!
//! let mut tls = TlsClient::new(transport, &mut read_buf, &mut write_buf);
//! tls.connect("example.local", &mut SoftwareRng::seed_from_u64(42))?;
//! tls.write_all(b"GET / HTTP/1.1\r\n\r\n")?;
//! ```

#![no_std]
#![warn(missing_docs)]

pub use embedded_tls::blocking::{
    Aes128GcmSha256, NoVerify, TlsConfig, TlsConnection, TlsContext, TlsError, UnsecureProvider,
};

use embedded_io::{ErrorType, Read, Write};
use rand_core::CryptoRngCore;

/// TLS 1.3 client session over a blocking `embedded-io` transport.
pub struct TlsClient<'a, T, Cipher = Aes128GcmSha256>
where
    T: Read + Write,
    Cipher: embedded_tls::blocking::TlsCipherSuite + 'static,
{
    inner: TlsConnection<'a, T, Cipher>,
}

impl<'a, T, Cipher> TlsClient<'a, T, Cipher>
where
    T: Read + Write,
    Cipher: embedded_tls::blocking::TlsCipherSuite + 'static,
{
    /// Create a client; call [`Self::connect`] before application data.
    pub fn new(transport: T, read_buf: &'a mut [u8], write_buf: &'a mut [u8]) -> Self {
        Self {
            inner: TlsConnection::new(transport, read_buf, write_buf),
        }
    }

    /// Perform TLS 1.3 handshake (no certificate verification — LAN lab use).
    pub fn connect<R: CryptoRngCore>(
        &mut self,
        server_name: &str,
        rng: &mut R,
    ) -> Result<(), TlsError> {
        let config = TlsConfig::new().with_server_name(server_name);
        let provider = UnsecureProvider::new::<Cipher>(rng);
        self.inner.open(TlsContext::new(&config, provider))
    }

    /// Borrow the underlying transport (post-handshake, still encrypted wrapper).
    pub fn inner_mut(&mut self) -> &mut TlsConnection<'a, T, Cipher> {
        &mut self.inner
    }
}

impl<'a, T, Cipher> Read for TlsClient<'a, T, Cipher>
where
    T: Read + Write,
    Cipher: embedded_tls::blocking::TlsCipherSuite + 'static,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.inner.read(buf)
    }
}

impl<'a, T, Cipher> Write for TlsClient<'a, T, Cipher>
where
    T: Read + Write,
    Cipher: embedded_tls::blocking::TlsCipherSuite + 'static,
{
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

impl<'a, T, Cipher> ErrorType for TlsClient<'a, T, Cipher>
where
    T: Read + Write,
    Cipher: embedded_tls::blocking::TlsCipherSuite + 'static,
{
    type Error = TlsError;
}
