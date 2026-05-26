//! MII / MDIO bit-bang via ETH MAC SMI.
#![allow(missing_docs)]

pub use ieee802_3_miim::Miim;

use crate::eth::mac::EthernetMAC;
use crate::pac::ethernet_mac::MACMIIAR;
use crate::pac::ETHERNET_MAC;

/// MDIO pin types (configured before use).
///
/// # Safety
/// Only pins specified as ETH_MDIO in the RM may implement this trait.
pub unsafe trait MdioPin {}

/// MDC pin types (configured before use).
///
/// # Safety
/// Only pins specified as ETH_MDC in the RM may implement this trait.
pub unsafe trait MdcPin {}

use crate::eth::setup::{Mdc, Mdio};

unsafe impl MdioPin for Mdio {}
unsafe impl MdcPin for Mdc {}

#[inline(always)]
fn miim_wait_ready(iar: &MACMIIAR) {
    while iar.read().mb().bit_is_set() {}
}

#[inline(always)]
fn miim_write(eth_mac: &mut ETHERNET_MAC, phy: u8, reg: u8, data: u16) {
    miim_wait_ready(&eth_mac.macmiiar);
    eth_mac.macmiidr.write(|w| w.md().bits(data));
    miim_wait_ready(&eth_mac.macmiiar);
    eth_mac.macmiiar.modify(|_, w| {
        w.pa()
            .bits(phy)
            .mr()
            .bits(reg)
            .mw()
            .set_bit()
            .mb()
            .set_bit()
    });
    miim_wait_ready(&eth_mac.macmiiar);
}

#[inline(always)]
fn miim_read(eth_mac: &mut ETHERNET_MAC, phy: u8, reg: u8) -> u16 {
    miim_wait_ready(&eth_mac.macmiiar);
    eth_mac.macmiiar.modify(|_, w| {
        w.pa()
            .bits(phy)
            .mr()
            .bits(reg)
            .mw()
            .clear_bit()
            .mb()
            .set_bit()
    });
    miim_wait_ready(&eth_mac.macmiiar);
    eth_mac.macmiidr.read().md().bits()
}

/// Serial Management Interface borrowing the MAC and MII marker pins.
pub struct Stm32Mii<'mac, 'pins, Mdio, Mdc> {
    mac: &'mac mut EthernetMAC,
    _mdio: &'pins mut Mdio,
    _mdc: &'pins mut Mdc,
}

impl<'mac, 'pins, Mdio, Mdc> Stm32Mii<'mac, 'pins, Mdio, Mdc>
where
    Mdio: MdioPin,
    Mdc: MdcPin,
{
    pub fn new(mac: &'mac mut EthernetMAC, _mdio: &'pins mut Mdio, _mdc: &'pins mut Mdc) -> Self {
        Self { mac, _mdio, _mdc }
    }

    pub fn read(&mut self, phy: u8, reg: u8) -> u16 {
        miim_read(&mut self.mac.eth_mac, phy, reg)
    }

    pub fn write(&mut self, phy: u8, reg: u8, data: u16) {
        miim_write(&mut self.mac.eth_mac, phy, reg, data);
    }
}

impl<'eth, 'pins, Mdio, Mdc> Miim for Stm32Mii<'eth, 'pins, Mdio, Mdc>
where
    Mdio: MdioPin,
    Mdc: MdcPin,
{
    fn read(&mut self, phy: u8, reg: u8) -> u16 {
        self.read(phy, reg)
    }

    fn write(&mut self, phy: u8, reg: u8, data: u16) {
        self.write(phy, reg, data);
    }
}
