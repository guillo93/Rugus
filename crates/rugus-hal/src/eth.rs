//! Ethernet MAC trait (G4) — implemented by chip HALs for smoltcp.

/// Link-layer MAC interface consumed by `rugus-net` / smoltcp.
///
/// Chip HALs implement this on top of their ETH driver + PHY.
pub trait EthMac {
    /// Driver-specific error type.
    type Error;

    /// Six-byte station address programmed in the MAC.
    fn mac_address(&self) -> [u8; 6];

    /// `true` when the PHY reports link up.
    fn link_up(&mut self) -> Result<bool, Self::Error>;
}
