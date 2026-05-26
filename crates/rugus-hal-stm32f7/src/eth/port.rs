//! `EthMac` trait adapter for STM32F7 Ethernet + LAN8742 PHY.

use core::convert::Infallible;

use ieee802_3_miim::phy::lan87xxa::LAN8742A;
use ieee802_3_miim::Phy;
use rugus_hal::EthMac;

use crate::eth::mac::EthernetMACWithMii;
use crate::eth::miim::{MdcPin, MdioPin};
use crate::eth::DEFAULT_MAC;

/// PHY wrapper implementing [`EthMac`].
pub struct EthMacPort<'a, MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    phy: &'a mut LAN8742A<EthernetMACWithMii<MDIO, MDC>>,
}

impl<'a, MDIO, MDC> EthMacPort<'a, MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    /// Wrap initialized PHY.
    pub fn new(phy: &'a mut LAN8742A<EthernetMACWithMii<MDIO, MDC>>) -> Self {
        Self { phy }
    }
}

impl<MDIO, MDC> EthMac for EthMacPort<'_, MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    type Error = Infallible;

    fn mac_address(&self) -> [u8; 6] {
        DEFAULT_MAC
    }

    fn link_up(&mut self) -> Result<bool, Self::Error> {
        Ok(self.phy.phy_link_up())
    }
}
