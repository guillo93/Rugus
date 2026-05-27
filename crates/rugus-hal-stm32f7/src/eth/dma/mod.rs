//! Ethernet DMA access and configuration.
#![allow(missing_docs, dead_code)]

use cortex_m::peripheral::NVIC;

use core::sync::atomic::{AtomicU32, Ordering};

use crate::cache;
use crate::pac::{Interrupt, ETHERNET_DMA, ETHERNET_MAC, ETHERNET_MMC};

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

static RX_FRAMES: AtomicU32 = AtomicU32::new(0);
static TX_FRAMES: AtomicU32 = AtomicU32::new(0);

/// Runtime counters and DMA status for RTT debug.
#[derive(Clone, Copy, Debug)]
pub struct EthStats {
    /// Frames delivered to the stack.
    pub rx_frames: u32,
    /// Frames submitted to TX DMA.
    pub tx_frames: u32,
    /// RX DMA process state (DMACSR RPS).
    pub rx_dma_state: u8,
    /// TX DMA process state (DMACSR TPS).
    pub tx_dma_state: u8,
    /// Receive buffer unavailable (RBUS).
    pub rx_buf_unavail: bool,
    /// Transmit buffer unavailable (TBUS).
    pub tx_buf_unavail: bool,
    /// Abnormal interrupt summary (AIS).
    pub abnormal_summary: bool,
    /// DMAOMR SR bit (RX DMA enable).
    pub rx_dma_enabled: bool,
    /// DMAOMR ST bit (TX DMA enable).
    pub tx_dma_enabled: bool,
}

/// Snapshot frame counters and DMA status registers.
pub fn eth_stats(dma: &EthernetDMA<'_, '_>) -> EthStats {
    let status = dma.eth_dma.dmasr.read();
    let omr = dma.eth_dma.dmaomr.read();
    EthStats {
        rx_frames: RX_FRAMES.load(Ordering::Relaxed),
        tx_frames: TX_FRAMES.load(Ordering::Relaxed),
        rx_dma_state: status.rps().bits(),
        tx_dma_state: status.tps().bits(),
        rx_buf_unavail: status.rbus().bit_is_set(),
        tx_buf_unavail: status.tbus().bit_is_set(),
        abnormal_summary: status.ais().bit_is_set(),
        rx_dma_enabled: omr.sr().bit_is_set(),
        tx_dma_enabled: omr.st().bit_is_set(),
    }
}

/// Key MAC/DMA registers for RTT debug (post-init or during traffic test).
#[derive(Clone, Copy, Debug)]
pub struct EthRegSnapshot {
    /// MACCR
    pub maccr: u32,
    /// DMABMR
    pub dmabmr: u32,
    /// DMASR
    pub dmasr: u32,
    /// DMAOMR
    pub dmaomr: u32,
    /// MMC good RX unicast frames (hardware counter).
    pub mmc_rx_unicast: u32,
    /// MMC TX good frames (hardware counter).
    pub mmc_tx_good: u32,
}

/// Read MAC/DMA control registers (safe anytime).
pub fn eth_regs(dma: &EthernetDMA<'_, '_>) -> EthRegSnapshot {
    let mac = unsafe { &*ETHERNET_MAC::ptr() };
    let mmc = unsafe { &*ETHERNET_MMC::ptr() };
    EthRegSnapshot {
        maccr: mac.maccr.read().bits(),
        dmabmr: dma.eth_dma.dmabmr.read().bits(),
        dmasr: dma.eth_dma.dmasr.read().bits(),
        dmaomr: dma.eth_dma.dmaomr.read().bits(),
        mmc_rx_unicast: mmc.mmcrgufcr.read().bits(),
        mmc_tx_good: mmc.mmctgfcr.read().bits(),
    }
}

pub(crate) fn note_rx_frame() {
    RX_FRAMES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_tx_frame() {
    TX_FRAMES.fetch_add(1, Ordering::Relaxed);
}

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

        // Clear sticky DMA status flags before descriptor programming (W1C).
        let pending = eth_dma.dmasr.read().bits();
        if pending != 0 {
            eth_dma.dmasr.write(|w| unsafe { w.bits(pending) });
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

        eth_dma.dmabmr.modify(|_, w| unsafe {
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
        });

        let dma = EthernetDMA {
            eth_dma,
            rx_ring: RxRing::new(rx_buffer),
            tx_ring: TxRing::new(tx_buffer),
        };

        dma
    }

    /// Start RX/TX descriptor rings. Call after [`crate::eth::init`] (MAC RE/TE set).
    pub fn start(&mut self) {
        // SAFETY: re-assert ETH MPU region before touching `.eth_dma` descriptors.
        unsafe {
            cache::configure_eth_mpu(&mut cortex_m::Peripherals::steal().MPU);
        }
        self.rx_ring.start(&self.eth_dma);
        self.tx_ring.start(&self.eth_dma);
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

    /// Re-arm RX/TX rings after PHY link-up (REF_CLK absent before autoneg).
    pub fn restart_after_link_up(&mut self) {
        self.eth_dma
            .dmaomr
            .modify(|_, w| w.sr().clear_bit().st().clear_bit());
        self.start();
        self.service_dma();
    }

    /// Clear RBUS/TBUS and poke RX/TX poll demand so DMA leaves stopped state.
    pub fn service_dma(&mut self) {
        let status = self.eth_dma.dmasr.read();
        if status.rbus().bit_is_set() {
            self.eth_dma
                .dmasr
                .write(|w| w.rbus().set_bit().nis().set_bit());
        }
        if status.tbus().bit_is_set() {
            self.eth_dma
                .dmasr
                .write(|w| w.tbus().set_bit().nis().set_bit());
            self.tx_ring.demand_poll();
        }
        self.rx_ring.demand_poll();
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
