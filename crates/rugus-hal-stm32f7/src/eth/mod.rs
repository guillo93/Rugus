//! Ethernet — ETH MAC + LAN8742A PHY (RMII) on STM32F769I-DISCO.
#![allow(missing_docs, dead_code)] // G4 step 1: internal DMA types; docs follow in step 2.

mod dma;
mod mac;
mod miim;
mod port;
mod setup;

pub use dma::{
    EthRxToken, EthTxToken, EthernetDMA, InterruptReasonSummary, RxError, RxRingEntry, TxError,
    TxRingEntry,
};
pub use mac::{EthernetMAC, EthernetMACWithMii, Speed, WrongClock};
pub use miim::{MdcPin, MdioPin, Miim, Stm32Mii};
pub use port::EthMacPort;
pub use setup::{configure_disco_pins, enable_peripheral, Mdc, Mdio, PartsIn, LAN8742_PHY_ADDR};

use ieee802_3_miim::Phy;

use crate::rcc::Clocks;

/// Default locally-administered MAC for Rugus F769 examples.
pub const DEFAULT_MAC: [u8; 6] = [0x02, 0x00, 0x52, 0x55, 0x47, 0x01];

/// Initialized Ethernet MAC + DMA + MII (MDIO/MDC configured).
pub struct EthStack<'rx, 'tx> {
    /// DMA engine (also the smoltcp `Device`).
    pub dma: EthernetDMA<'rx, 'tx>,
    /// MAC + MII for PHY access.
    pub mac: EthernetMACWithMii<Mdio, Mdc>,
}

/// Create MAC + DMA. Call [`configure_disco_pins`] + [`enable_peripheral`] first.
pub fn init<'rx, 'tx>(
    parts: PartsIn,
    clocks: &Clocks,
    rx_ring: &'rx mut [RxRingEntry],
    tx_ring: &'tx mut [TxRingEntry],
) -> Result<EthStack<'rx, 'tx>, WrongClock> {
    let PartsIn { mac, mmc, dma } = parts;

    let dma = EthernetDMA::new(dma, rx_ring, tx_ring);
    let mut mac =
        EthernetMAC::new(mac, mmc, clocks, Speed::FullDuplexBase100Tx, &dma)?.with_mii(Mdio, Mdc);
    mac.set_mac_address(DEFAULT_MAC);

    Ok(EthStack { dma, mac })
}

/// Initialize LAN8742A (reset + autoneg).
pub fn init_phy<M: Miim>(phy: &mut ieee802_3_miim::phy::lan87xxa::LAN8742A<M>) {
    phy.reset();
    phy.phy_init();
}

/// Poll PHY link status.
pub fn link_up<M: Miim>(phy: &mut ieee802_3_miim::phy::lan87xxa::LAN8742A<M>) -> bool {
    phy.phy_link_up()
}

/// Handle `ETH` IRQ — call from `#[pac::interrupt]` handler.
pub fn eth_interrupt_handler() -> InterruptReasonSummary {
    EthernetDMA::interrupt_handler()
}

/// Enable NVIC `ETH` and DMA RX/TX IRQs.
pub fn enable_eth_interrupt(dma: &EthernetDMA<'_, '_>) {
    dma.enable_interrupt();
}
