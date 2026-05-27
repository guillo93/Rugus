use super::PacketId;
use crate::pac::ETHERNET_DMA;

mod descriptor;
pub use descriptor::{TxDescriptor, TxRingEntry};

/// Errors that can occur during Ethernet TX
#[derive(Debug, PartialEq)]
pub enum TxError {
    /// Ring buffer is full
    WouldBlock,
}

/// Tx DMA state
pub struct TxRing<'a> {
    entries: &'a mut [TxRingEntry],
    next_entry: usize,
}

impl<'ring> TxRing<'ring> {
    /// Allocate
    ///
    /// `start()` will be needed before `send()`
    pub(crate) fn new(entries: &'ring mut [TxRingEntry]) -> Self {
        TxRing {
            entries,
            next_entry: 0,
        }
    }

    /// Start the Tx DMA engine
    pub(crate) fn start(&mut self, eth_dma: &ETHERNET_DMA) {
        {
            let first_ptr = self.entries[0].desc() as *const TxDescriptor;
            let mut previous: Option<&mut TxRingEntry> = None;
            for entry in self.entries.iter_mut() {
                let current_ptr = entry.desc() as *const TxDescriptor;
                if let Some(prev_entry) = &mut previous {
                    prev_entry.setup(current_ptr);
                }
                previous = Some(entry);
            }
            if let Some(entry) = &mut previous {
                entry.setup(first_ptr);
            }
        }

        let ring_ptr = self.entries[0].desc() as *const TxDescriptor;
        eth_dma
            .dmatdlar
            .write(|w| unsafe { w.stl().bits(ring_ptr as u32) });

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        eth_dma.dmaomr.modify(|_, w| w.st().set_bit());
    }

    /// Stop the TX DMA
    pub(crate) fn stop(&self, eth_dma: &ETHERNET_DMA) {
        eth_dma.dmaomr.modify(|_, w| w.st().clear_bit());
        while self.is_running() {}
    }

    /// If this returns `true`, the next `send` will succeed.
    pub fn next_entry_available(&self) -> bool {
        self.entries[self.next_entry].is_available()
    }

    /// Check if we can send the next TX entry.
    fn send_next_impl(&mut self) -> Result<usize, TxError> {
        let entries_len = self.entries.len();
        let entry_num = self.next_entry;
        let entry = &mut self.entries[entry_num];

        if entry.is_available() {
            self.next_entry = (self.next_entry + 1) % entries_len;
            Ok(entry_num)
        } else {
            Err(TxError::WouldBlock)
        }
    }

    /// Prepare a packet for sending.
    pub fn send_next<'borrow>(
        &'borrow mut self,
        length: usize,
        packet_id: Option<PacketId>,
    ) -> Result<TxPacket<'borrow, 'ring>, TxError> {
        let entry = self.send_next_impl()?;
        let tx_buffer = self.entries[entry].buffer_mut();

        assert!(length <= tx_buffer.len(), "Not enough space in TX buffer");

        Ok(TxPacket {
            ring: self,
            idx: entry,
            length,
            packet_id,
        })
    }

    /// Whether the TX DMA engine is running.
    pub fn is_running(&self) -> bool {
        self.running_state().is_running()
    }

    /// Current TX DMA run state.
    pub fn running_state(&self) -> RunningState {
        let eth_dma = unsafe { &*ETHERNET_DMA::ptr() };
        match eth_dma.dmasr.read().tps().bits() {
            0b000 | 0b110 => RunningState::Stopped,
            0b001..=0b011 => RunningState::Running,
            0b100 => RunningState::Suspended,
            0b101 => RunningState::Reserved,
            _ => RunningState::Unknown,
        }
    }

    pub(crate) fn demand_poll(&self) {
        let eth_dma = unsafe { &*ETHERNET_DMA::ptr() };
        eth_dma.dmasr.write(|w| w.tbus().set_bit());
        eth_dma.dmatpdr.write(|w| unsafe { w.tpd().bits(1) });
    }
}

#[derive(Debug, PartialEq)]
/// The run state of the TX DMA.
pub enum RunningState {
    /// Reset or Stop Transmit Command issued
    Stopped,
    /// Fetching transmit transfer descriptor
    Running,
    /// Reserved for future use
    Reserved,
    /// Transmit descriptor unavailable
    Suspended,
    /// Invalid value
    Unknown,
}

impl RunningState {
    /// Check whether this state represents that the TX DMA is running
    pub fn is_running(&self) -> bool {
        *self == RunningState::Running
    }
}

/// A struct that represents a soon-to-be-sent packet.
pub struct TxPacket<'borrow, 'ring> {
    ring: &'borrow mut TxRing<'ring>,
    idx: usize,
    length: usize,
    packet_id: Option<PacketId>,
}

impl core::ops::Deref for TxPacket<'_, '_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.ring.entries[self.idx].buffer()[..self.length]
    }
}

impl core::ops::DerefMut for TxPacket<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ring.entries[self.idx].buffer_mut()[..self.length]
    }
}

impl TxPacket<'_, '_> {
    /// Send this packet!
    pub fn send(self) {
        drop(self);
    }
}

impl Drop for TxPacket<'_, '_> {
    fn drop(&mut self) {
        self.ring.entries[self.idx].send(self.length, self.packet_id);
        self.ring.demand_poll();
    }
}
