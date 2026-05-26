use core::ops::{Deref, DerefMut};

use aligned::{Aligned, A8};
use volatile_register::{RO, RW};

use crate::cache;

const DESC_SIZE: usize = 8;

#[repr(C)]
pub struct Descriptor {
    pub(crate) desc: Aligned<A8, [u32; DESC_SIZE]>,
}

impl Clone for Descriptor {
    fn clone(&self) -> Self {
        Descriptor {
            desc: Aligned(*self.desc),
        }
    }
}

impl Default for Descriptor {
    fn default() -> Self {
        Self::new()
    }
}

impl Descriptor {
    pub const fn new() -> Self {
        Self {
            desc: Aligned([0; DESC_SIZE]),
        }
    }

    pub unsafe fn clear(&mut self) {
        for i in 0..DESC_SIZE {
            // SAFETY: caller ensures descriptor is valid for DMA setup.
            unsafe {
                self.write(i, 0);
            }
        }
    }

    fn r(&self, n: usize) -> &RO<u32> {
        let ro = &self.desc.deref()[n] as *const _ as *const RO<u32>;
        unsafe { &*ro }
    }

    unsafe fn rw(&mut self, n: usize) -> &mut RW<u32> {
        let rw = &mut self.desc.deref_mut()[n] as *mut _ as *mut RW<u32>;
        // SAFETY: index bounded by DESC_SIZE; aliasing matches stm32-eth layout.
        unsafe { &mut *rw }
    }

    pub(crate) fn invalidate_cpu(&self) {
        let bytes = self.desc.deref();
        let len = DESC_SIZE * core::mem::size_of::<u32>();
        cache::invalidate_dcache_for_dma(unsafe {
            core::slice::from_raw_parts(bytes.as_ptr() as *const u8, len)
        });
    }

    pub(crate) fn clean_dma(&self) {
        let bytes = self.desc.deref();
        let len = DESC_SIZE * core::mem::size_of::<u32>();
        cache::clean_dcache_for_dma(unsafe {
            core::slice::from_raw_parts(bytes.as_ptr() as *const u8, len)
        });
    }

    pub fn read(&self, n: usize) -> u32 {
        self.r(n).read()
    }

    pub unsafe fn write(&mut self, n: usize, value: u32) {
        self.clean_dma();
        // SAFETY: n < DESC_SIZE; rw points at descriptor word.
        unsafe {
            self.rw(n).write(value);
        }
        self.clean_dma();
    }

    pub unsafe fn modify<F>(&mut self, n: usize, f: F)
    where
        F: FnOnce(u32) -> u32,
    {
        // SAFETY: n < DESC_SIZE; rw points at descriptor word.
        unsafe {
            self.rw(n).modify(f);
        }
        self.clean_dma();
    }
}
