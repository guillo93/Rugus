#![allow(missing_docs)]

pub(crate) use self::descriptor::RxDescriptor;

use self::descriptor::RxDescriptorError;
pub use self::descriptor::RxRingEntry;

use super::PacketId;
use crate::pac::ETHERNET_DMA;

mod descriptor;

#[derive(Debug, PartialEq)]
pub enum RxError {
    Truncated,
    DmaError,
    WouldBlock,
}

impl From<RxDescriptorError> for RxError {
    fn from(value: RxDescriptorError) -> Self {
        match value {
            RxDescriptorError::Truncated => Self::Truncated,
            RxDescriptorError::DmaError => Self::DmaError,
        }
    }
}

pub struct RxRing<'a> {
    entries: &'a mut [RxRingEntry],
    next_entry: usize,
}

impl<'a> RxRing<'a> {
    pub(crate) fn new(entries: &'a mut [RxRingEntry]) -> Self {
        RxRing {
            entries,
            next_entry: 0,
        }
    }

    pub(crate) fn start(&mut self, eth_dma: &ETHERNET_DMA) {
        {
            let first_ptr = self.entries[0].desc() as *const RxDescriptor;
            let mut previous: Option<&mut RxRingEntry> = None;
            for entry in self.entries.iter_mut() {
                let current_ptr = entry.desc() as *const RxDescriptor;
                if let Some(prev_entry) = &mut previous {
                    prev_entry.setup(current_ptr);
                }
                previous = Some(entry);
            }
            if let Some(entry) = &mut previous {
                entry.setup(first_ptr);
            }
        }
        self.next_entry = 0;
        let ring_ptr = self.entries[0].desc() as *const RxDescriptor;

        eth_dma
            .dmardlar
            .write(|w| unsafe { w.srl().bits(ring_ptr as u32) });

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        eth_dma.dmaomr.modify(|_, w| w.sr().set_bit());
        self.demand_poll();
    }

    pub(crate) fn stop(&self, eth_dma: &ETHERNET_DMA) {
        eth_dma.dmaomr.modify(|_, w| w.sr().clear_bit());
        while self.running_state().is_running() {}
    }

    pub(crate) fn demand_poll(&self) {
        let eth_dma = unsafe { &*ETHERNET_DMA::ptr() };
        eth_dma.dmasr.write(|w| w.rbus().set_bit());
        eth_dma.dmarpdr.write(|w| unsafe { w.rpd().bits(1) });
    }

    pub fn running_state(&self) -> RunningState {
        let eth_dma = unsafe { &*ETHERNET_DMA::ptr() };
        match eth_dma.dmasr.read().rps().bits() {
            0b000 | 0b100 => RunningState::Stopped,
            0b001 | 0b011 | 0b101..=0b111 => RunningState::Running,
            _ => RunningState::Unknown,
        }
    }

    pub fn next_entry_available(&mut self) -> bool {
        if !self.running_state().is_running() {
            self.demand_poll();
        }

        loop {
            let entry = &mut self.entries[self.next_entry];
            if !entry.is_available() {
                return false;
            }
            if !entry.is_valid() {
                entry.discard();
                self.next_entry = (self.next_entry + 1) % self.entries.len();
                continue;
            }
            return true;
        }
    }

    fn recv_next_impl(
        &mut self,
        #[allow(unused_variables)] packet_id: Option<PacketId>,
    ) -> Result<(usize, usize), RxError> {
        if !self.running_state().is_running() {
            self.demand_poll();
        }

        let entries_len = self.entries.len();
        let entry_num = self.next_entry;
        let entry = &mut self.entries[entry_num];

        if entry.is_available() {
            self.next_entry = (self.next_entry + 1) % entries_len;
            match entry.recv(packet_id) {
                Ok(length) => {
                    super::note_rx_frame();
                    Ok((entry_num, length))
                }
                Err(e) => Err(e.into()),
            }
        } else {
            Err(RxError::WouldBlock)
        }
    }

    pub fn recv_next(&'_ mut self, packet_id: Option<PacketId>) -> Result<RxPacket<'_>, RxError> {
        let (entry, length) = self.recv_next_impl(packet_id)?;
        Ok(RxPacket {
            entry: &mut self.entries[entry],
            length,
        })
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum RunningState {
    Unknown,
    Stopped,
    Running,
}

impl RunningState {
    pub fn is_running(&self) -> bool {
        *self == RunningState::Running
    }
}

pub struct RxPacket<'a> {
    entry: &'a mut RxRingEntry,
    length: usize,
}

impl<'a> core::ops::Deref for RxPacket<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.entry.as_slice()[0..self.length]
    }
}

impl<'a> core::ops::DerefMut for RxPacket<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry.as_mut_slice()[0..self.length]
    }
}

impl<'a> Drop for RxPacket<'a> {
    fn drop(&mut self) {
        self.entry.desc_mut().set_owned();
    }
}

impl<'a> RxPacket<'a> {
    pub fn free(self) {}
}
