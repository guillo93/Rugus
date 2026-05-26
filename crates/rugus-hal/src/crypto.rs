//! Crypto trait (G4) — backends live in `rugus-crypto` and chip HALs.
//!
//! STM32F7 CRYP/HASH/RNG hardware is planned; until then use
//! `SoftwareRng` from `rugus-crypto` and pure-Rust digests.

/// Fill a buffer with cryptographically suitable random bytes.
pub trait CryptoRng {
    /// Driver-specific error.
    type Error;

    /// Write random bytes into `buf`.
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), Self::Error>;
}
