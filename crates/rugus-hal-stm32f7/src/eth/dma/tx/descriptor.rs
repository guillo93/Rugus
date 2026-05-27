use super::super::desc::Descriptor;
use super::super::ring::{RingDescriptor, RingEntry};
use super::super::PacketId;
use crate::cache;

const TXDESC_0_OWN: u32 = 1 << 31;
const TXDESC_0_IC: u32 = 1 << 30;
const TXDESC_0_FS: u32 = 1 << 28;
const TXDESC_0_LS: u32 = 1 << 29;
const TXDESC_0_CIC0: u32 = 1 << 23;
const TXDESC_0_CIC1: u32 = 1 << 22;
const TXDESC_0_TER: u32 = 1 << 21;
const TXDESC_0_TCH: u32 = 1 << 20;

const TXDESC_1_TBS_SHIFT: usize = 0;
const TXDESC_1_TBS_MASK: u32 = 0x0fff << TXDESC_1_TBS_SHIFT;

#[repr(C)]
pub struct TxDescriptor {
    desc: Descriptor,
    packet_id: Option<PacketId>,
    buffer1: u32,
    next_descriptor: u32,
    is_last: bool,
}

impl Default for TxDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

impl TxDescriptor {
    pub const fn new() -> Self {
        Self {
            desc: Descriptor::new(),
            packet_id: None,
            buffer1: 0,
            next_descriptor: 0,
            is_last: false,
        }
    }

    fn is_owned(&self) -> bool {
        self.desc.invalidate_cpu();
        (self.desc.read(0) & TXDESC_0_OWN) == TXDESC_0_OWN
    }

    fn set_owned(&mut self, length: usize, packet_id: Option<PacketId>) {
        self.packet_id = packet_id;
        self.set_buffer1_len(length);

        unsafe {
            self.desc.write(2, self.buffer1);
            self.desc.write(3, self.next_descriptor);
        }

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
        cortex_m::asm::dsb();

        let extra_flags = 0u32;

        unsafe {
            self.desc.write(
                0,
                TXDESC_0_OWN | TXDESC_0_TCH | TXDESC_0_FS | TXDESC_0_LS | TXDESC_0_IC | extra_flags,
            );
        }

        cortex_m::asm::dsb();
    }

    fn set_buffer1_len(&mut self, len: usize) {
        unsafe {
            self.desc.modify(1, |w| {
                (w & !TXDESC_1_TBS_MASK) | ((len as u32) << TXDESC_1_TBS_SHIFT)
            });
        }
    }
}

pub type TxRingEntry = RingEntry<TxDescriptor>;

impl RingDescriptor for TxDescriptor {
    fn setup(&mut self, buffer: *const u8, _len: usize, next: *const Self) {
        unsafe {
            self.desc.clear();
        }

        let next_desc_addr = unsafe { &(*next).desc as *const Descriptor as *const u8 as u32 };

        self.buffer1 = buffer as u32;
        self.next_descriptor = next_desc_addr;

        // is_last is no longer needed since we form a true ring via next_descriptor.
        self.is_last = false;
    }
}

impl TxRingEntry {
    pub(super) fn is_available(&self) -> bool {
        !self.desc().is_owned()
    }

    pub(super) fn send(&mut self, length: usize, packet_id: Option<PacketId>) {
        cache::clean_dcache_for_dma(&self.as_slice()[..length]);
        self.desc_mut().set_owned(length, packet_id);
        super::super::note_tx_frame();
    }

    pub fn buffer(&self) -> &[u8] {
        self.as_slice()
    }

    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}
