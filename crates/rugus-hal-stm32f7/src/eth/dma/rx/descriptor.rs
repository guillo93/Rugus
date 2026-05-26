use super::super::desc::Descriptor;
use super::super::ring::{RingDescriptor, RingEntry};
use super::super::PacketId;
use crate::cache;

#[derive(Debug, PartialEq)]
pub(crate) enum RxDescriptorError {
    Truncated,
    DmaError,
}

const RXDESC_0_OWN: u32 = 1 << 31;
const RXDESC_0_FS: u32 = 1 << 9;
const RXDESC_0_LS: u32 = 1 << 8;
const RXDESC_0_ES: u32 = 1 << 15;
const RXDESC_0_FL_MASK: u32 = 0x3FFF;
const RXDESC_0_FL_SHIFT: usize = 16;

const RXDESC_1_RBS_SHIFT: usize = 0;
const RXDESC_1_RBS_MASK: u32 = 0x0fff << RXDESC_1_RBS_SHIFT;
const RXDESC_1_RCH: u32 = 1 << 14;
const RXDESC_1_RER: u32 = 1 << 15;

#[repr(C)]
pub struct RxDescriptor {
    desc: Descriptor,
    buffer1: Option<u32>,
    next_descriptor: Option<u32>,
    packet_id: Option<PacketId>,
}

impl Default for RxDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

impl RxDescriptor {
    pub const fn new() -> Self {
        Self {
            desc: Descriptor::new(),
            buffer1: None,
            next_descriptor: None,
            packet_id: None,
        }
    }

    fn is_owned(&self) -> bool {
        self.desc.invalidate_cpu();
        (self.desc.read(0) & RXDESC_0_OWN) == RXDESC_0_OWN
    }

    pub fn set_owned(&mut self) {
        self.write_buffer1();
        self.write_buffer2();
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
        unsafe {
            self.desc.write(0, RXDESC_0_OWN);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }

    fn has_error(&self) -> bool {
        (self.desc.read(0) & RXDESC_0_ES) == RXDESC_0_ES
    }

    fn is_first(&self) -> bool {
        (self.desc.read(0) & RXDESC_0_FS) == RXDESC_0_FS
    }

    fn is_last(&self) -> bool {
        (self.desc.read(0) & RXDESC_0_LS) == RXDESC_0_LS
    }

    fn write_buffer1(&mut self) {
        let buffer_addr = self.buffer1.expect("RX descriptor buffer1 unset");
        unsafe {
            self.desc.write(2, buffer_addr);
        }
    }

    fn set_buffer1(&mut self, buffer: *const u8, len: usize) {
        self.buffer1 = Some(buffer as u32);
        self.write_buffer1();
        unsafe {
            self.desc.modify(1, |w| {
                (w & !RXDESC_1_RBS_MASK) | ((len as u32) << RXDESC_1_RBS_SHIFT)
            });
        }
    }

    fn write_buffer2(&mut self) {
        let addr = self.next_descriptor.expect("RX descriptor next unset");
        unsafe {
            self.desc.write(3, addr);
        }
    }

    fn set_buffer2(&mut self, buffer: *const u8) {
        self.next_descriptor = Some(buffer as u32);
        self.write_buffer2();
    }

    fn set_end_of_ring(&mut self) {
        unsafe {
            self.desc.modify(1, |w| w | RXDESC_1_RER);
        }
    }

    fn get_frame_len(&self) -> usize {
        ((self.desc.read(0) >> RXDESC_0_FL_SHIFT) & RXDESC_0_FL_MASK) as usize
    }
}

pub type RxRingEntry = RingEntry<RxDescriptor>;

impl RingDescriptor for RxDescriptor {
    fn setup(&mut self, buffer: *const u8, len: usize, next: Option<&Self>) {
        unsafe {
            self.desc.write(1, RXDESC_1_RCH);
        }
        self.set_buffer1(buffer, len);
        match next {
            Some(next) => self.set_buffer2(&next.desc as *const Descriptor as *const u8),
            None => {
                self.set_buffer2(core::ptr::null());
                self.set_end_of_ring();
            }
        };
        self.set_owned();
    }
}

impl RxRingEntry {
    pub(super) fn is_available(&self) -> bool {
        !self.desc().is_owned()
    }

    pub(super) fn recv(&mut self, packet_id: Option<PacketId>) -> Result<usize, RxDescriptorError> {
        if self.desc().has_error() {
            self.desc_mut().set_owned();
            Err(RxDescriptorError::DmaError)
        } else if self.desc().is_first() && self.desc().is_last() {
            let frame_len = self.desc().get_frame_len();
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Acquire);
            cache::invalidate_dcache_for_dma(&self.as_slice()[..frame_len]);
            self.desc_mut().packet_id = packet_id;
            Ok(frame_len)
        } else {
            self.desc_mut().set_owned();
            Err(RxDescriptorError::Truncated)
        }
    }
}
