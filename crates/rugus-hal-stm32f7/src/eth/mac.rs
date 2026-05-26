//! Ethernet MAC access and configuration (adapted from stm32-eth, PAC-only).

use core::ops::{Deref, DerefMut};

use crate::eth::dma::EthernetDMA;
use crate::eth::miim::{MdcPin, MdioPin, Miim, Stm32Mii};
use crate::pac::{ETHERNET_MAC, ETHERNET_MMC};
use crate::rcc::Clocks;

/// Speeds at which this MAC can be configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    /// 10Base-T half duplex.
    HalfDuplexBase10T,
    /// 10Base-T full duplex.
    FullDuplexBase10T,
    /// 100Base-Tx half duplex.
    HalfDuplexBase100Tx,
    /// 100Base-Tx full duplex.
    FullDuplexBase100Tx,
}

mod consts {
    pub const ETH_MACMIIAR_CR_HCLK_DIV_42: u8 = 0;
    pub const ETH_MACMIIAR_CR_HCLK_DIV_62: u8 = 1;
    pub const ETH_MACMIIAR_CR_HCLK_DIV_16: u8 = 2;
    pub const ETH_MACMIIAR_CR_HCLK_DIV_26: u8 = 3;
    pub const ETH_MACMIIAR_CR_HCLK_DIV_102: u8 = 4;
}
use self::consts::*;

/// HCLK must be at least 25 MHz to use the ethernet peripheral.
#[derive(Debug)]
pub struct WrongClock;

/// Ethernet media access control (MAC).
pub struct EthernetMAC {
    pub(crate) eth_mac: ETHERNET_MAC,
}

impl EthernetMAC {
    pub(crate) fn new(
        eth_mac: ETHERNET_MAC,
        eth_mmc: ETHERNET_MMC,
        clocks: &Clocks,
        initial_speed: Speed,
        _dma: &EthernetDMA<'_, '_>,
    ) -> Result<Self, WrongClock> {
        let clock_frequency = clocks.hclk;

        let clock_range = match clock_frequency {
            0..=24_999_999 => return Err(WrongClock),
            25_000_000..=34_999_999 => ETH_MACMIIAR_CR_HCLK_DIV_16,
            35_000_000..=59_999_999 => ETH_MACMIIAR_CR_HCLK_DIV_26,
            60_000_000..=99_999_999 => ETH_MACMIIAR_CR_HCLK_DIV_42,
            100_000_000..=149_999_999 => ETH_MACMIIAR_CR_HCLK_DIV_62,
            150_000_000.. => ETH_MACMIIAR_CR_HCLK_DIV_102,
        };

        eth_mac
            .macmiiar
            .modify(|_, w| unsafe { w.cr().bits(clock_range) });

        eth_mac.maccr.modify(|_, w| {
            w.cstf()
                .set_bit()
                .fes()
                .set_bit()
                .dm()
                .set_bit()
                .ipco()
                .set_bit()
                .apcs()
                .set_bit()
                .rd()
                .set_bit()
                .re()
                .set_bit()
                .te()
                .set_bit()
        });

        eth_mac
            .macffr
            .modify(|_, w| w.ra().set_bit().pm().set_bit());

        eth_mac.macfcr.modify(|_, w| w.pt().bits(0x100));

        eth_mmc
            .mmcrimr
            .write(|w| w.rgufm().set_bit().rfaem().set_bit().rfcem().set_bit());

        eth_mmc
            .mmctimr
            .write(|w| w.tgfm().set_bit().tgfmscm().set_bit().tgfscm().set_bit());

        eth_mmc
            .mmctimr
            .modify(|r, w| unsafe { w.bits(r.bits() | (1 << 21)) });

        let mut me = Self { eth_mac };
        me.set_speed(initial_speed);
        Ok(me)
    }

    pub fn mii<'eth, 'pins, Mdio, Mdc>(
        &'eth mut self,
        mdio: &'pins mut Mdio,
        mdc: &'pins mut Mdc,
    ) -> Stm32Mii<'eth, 'pins, Mdio, Mdc>
    where
        Mdio: MdioPin,
        Mdc: MdcPin,
    {
        Stm32Mii::new(self, mdio, mdc)
    }

    pub fn with_mii<MDIO, MDC>(self, mdio: MDIO, mdc: MDC) -> EthernetMACWithMii<MDIO, MDC>
    where
        MDIO: MdioPin,
        MDC: MdcPin,
    {
        EthernetMACWithMii {
            eth_mac: self,
            mdio,
            mdc,
        }
    }

    pub fn set_mac_address(&mut self, mac: [u8; 6]) {
        let high = u16::from(mac[5]) << 8 | u16::from(mac[4]);
        let low = u32::from(mac[0])
            | (u32::from(mac[1]) << 8)
            | (u32::from(mac[2]) << 16)
            | (u32::from(mac[3]) << 24);
        self.eth_mac.maca0hr.modify(|_, w| w.maca0h().bits(high));
        self.eth_mac.maca0lr.write(|w| w.maca0l().bits(low));
    }

    /// Read station address MA0.
    pub fn mac_address(&self) -> [u8; 6] {
        let lo = self.eth_mac.maca0lr.read().maca0l().bits();
        let hi = self.eth_mac.maca0hr.read().maca0h().bits();
        [
            (lo & 0xFF) as u8,
            ((lo >> 8) & 0xFF) as u8,
            ((lo >> 16) & 0xFF) as u8,
            ((lo >> 24) & 0xFF) as u8,
            (hi & 0xFF) as u8,
            ((hi >> 8) & 0xFF) as u8,
        ]
    }

    pub fn set_speed(&mut self, speed: Speed) {
        self.eth_mac.maccr.modify(|_, w| match speed {
            Speed::HalfDuplexBase10T => w.fes().clear_bit().dm().clear_bit(),
            Speed::FullDuplexBase10T => w.fes().clear_bit().dm().set_bit(),
            Speed::HalfDuplexBase100Tx => w.fes().set_bit().dm().clear_bit(),
            Speed::FullDuplexBase100Tx => w.fes().set_bit().dm().set_bit(),
        });
    }
}

/// Ethernet MAC with owned MII pins (marker types after GPIO setup).
pub struct EthernetMACWithMii<MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    pub(crate) eth_mac: EthernetMAC,
    mdio: MDIO,
    mdc: MDC,
}

impl<MDIO, MDC> Deref for EthernetMACWithMii<MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    type Target = EthernetMAC;

    fn deref(&self) -> &Self::Target {
        &self.eth_mac
    }
}

impl<MDIO, MDC> DerefMut for EthernetMACWithMii<MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.eth_mac
    }
}

impl<MDIO, MDC> EthernetMACWithMii<MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    pub fn read(&mut self, phy: u8, reg: u8) -> u16 {
        self.eth_mac
            .mii(&mut self.mdio, &mut self.mdc)
            .read(phy, reg)
    }

    pub fn write(&mut self, phy: u8, reg: u8, data: u16) {
        self.eth_mac
            .mii(&mut self.mdio, &mut self.mdc)
            .write(phy, reg, data);
    }
}

impl<MDIO, MDC> Miim for EthernetMACWithMii<MDIO, MDC>
where
    MDIO: MdioPin,
    MDC: MdcPin,
{
    fn read(&mut self, phy: u8, reg: u8) -> u16 {
        self.read(phy, reg)
    }

    fn write(&mut self, phy: u8, reg: u8, data: u16) {
        self.write(phy, reg, data);
    }
}
