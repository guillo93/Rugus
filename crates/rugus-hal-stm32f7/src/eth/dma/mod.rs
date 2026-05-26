//! Ethernet DMA access and configuration.
#![allow(missing_docs, dead_code)]

use cortex_m::peripheral::NVIC;

use crate::pac::{Interrupt, ETHERNET_DMA};

mod smoltcp_phy;
pub use smoltcp_phy::*;

pub(crate) mod desc;
pub(crate) mod ring;

mod rx;
pub use rx::{RxError, RxPacket, RxRing, RxRingEntry};

mod tx;
pub use tx::{TxError, TxRing, TxRingEntry};

mod packet_id;
pub use packet_id::PacketId;

/// VLAN frame max size per datasheet.
pub(crate) const MTU: usize = 1522;

/// Packet ID not found in DMA descriptors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PacketIdNotFound;

/// Ethernet DMA engine.
pub struct EthernetDMA<'rx, 'tx> {
    pub(crate) eth_dma: ETHERNET_DMA,
    pub(crate) rx_ring: RxRing<'rx>,
    pub(crate) tx_ring: TxRing<'tx>,
}

impl<'rx, 'tx> EthernetDMA<'rx, 'tx> {
    pub(crate) fn new(
        eth_dma: ETHERNET_DMA,
        rx_buffer: &'rx mut [RxRingEntry],
        tx_buffer: &'tx mut [TxRingEntry],
    ) -> Self {
        eth_dma.dmabmr.modify(|_, w| w.sr().set_bit());
        for _ in 0..100_000 {
            if !eth_dma.dmabmr.read().sr().bit_is_set() {
                break;
            }
        }

        eth_dma.dmaomr.modify(|_, w| {
            w.dtcefd()
                .set_bit()
                .rsf()
                .set_bit()
                .dfrf()
                .set_bit()
                .tsf()
                .set_bit()
                .fef()
                .set_bit()
                .osf()
                .set_bit()
        });

        eth_dma.dmabmr.modify(|_, w| {
            let w = w.edfe().set_bit();
            unsafe {
                w.aab()
                    .set_bit()
                    .fb()
                    .set_bit()
                    .rdp()
                    .bits(32)
                    .pbl()
                    .bits(32)
                    .pm()
                    .bits(0b01)
                    .usp()
                    .set_bit()
            }
        });

        let mut dma = EthernetDMA {
            eth_dma,
            rx_ring: RxRing::new(rx_buffer),
            tx_ring: TxRing::new(tx_buffer),
        };

        dma.rx_ring.start(&dma.eth_dma);
        dma.tx_ring.start(&dma.eth_dma);
        dma
    }

    pub fn enable_interrupt(&self) {
        self.eth_dma
            .dmaier
            .modify(|_, w| w.nise().set_bit().rie().set_bit().tie().set_bit());
        unsafe {
            NVIC::unmask(Interrupt::ETH);
        }
    }

    pub fn interrupt_handler() -> InterruptReasonSummary {
        let eth_dma = unsafe { &*ETHERNET_DMA::ptr() };
        let status = eth_dma.dmasr.read();
        let status = InterruptReasonSummary {
            is_rx: status.rs().bit_is_set(),
            is_tx: status.ts().bit_is_set(),
            is_error: status.ais().bit_is_set(),
        };
        eth_dma
            .dmasr
            .write(|w| w.nis().set_bit().ts().set_bit().rs().set_bit());
        status
    }

    pub fn recv_next(&'_ mut self, packet_id: Option<PacketId>) -> Result<RxPacket<'_>, RxError> {
        self.rx_ring.recv_next(packet_id)
    }

    pub fn rx_is_running(&self) -> bool {
        self.rx_ring.running_state().is_running()
    }

    pub fn tx_is_running(&self) -> bool {
        self.tx_ring.is_running()
    }

    pub fn send<F>(
        &mut self,
        length: usize,
        packet_id: Option<PacketId>,
        f: F,
    ) -> Result<(), TxError>
    where
        F: FnOnce(&mut [u8]),
    {
        let mut tx_packet = self.tx_ring.send_next(length, packet_id)?;
        f(&mut tx_packet);
        tx_packet.send();
        Ok(())
    }

    pub fn rx_available(&mut self) -> bool {
        self.rx_ring.next_entry_available()
    }

    pub fn tx_available(&mut self) -> bool {
        self.tx_ring.next_entry_available()
    }
}

impl Drop for EthernetDMA<'_, '_> {
    fn drop(&mut self) {
        self.tx_ring.stop(&self.eth_dma);
        self.rx_ring.stop(&self.eth_dma);
    }
}

/// Summary of ETH DMA interrupt causes.
#[derive(Debug, Clone, Copy)]
pub struct InterruptReasonSummary {
    /// RX event pending.
    pub is_rx: bool,
    /// TX event pending.
    pub is_tx: bool,
    /// DMA error pending.
    pub is_error: bool,
}
