//! RCC — system clock for STM32F103 Blue Pill.
//!
//! Keeps the default **HSI 8 MHz** SYSCLK (no PLL). Reliable on clones with
//! missing or inaccurate HSE resonators; enough for blink + defmt RTT.

use crate::pac;

/// HSI frequency after reset (RM0008).
pub const HSI_HZ: u32 = 8_000_000;

/// Bus clocks after [`init`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    /// Core frequency (SYSCLK).
    pub sysclk: u32,
    /// AHB (HCLK), same as SYSCLK with default prescaler.
    pub hclk: u32,
    /// APB1 (PCLK1), max 36 MHz on F103.
    pub pclk1: u32,
    /// APB2 (PCLK2), max 72 MHz on F103.
    pub pclk2: u32,
}

impl Clocks {
    /// SYSCLK in megahertz for `defmt` logs.
    pub const fn sysclk_mhz(&self) -> u32 {
        self.sysclk / 1_000_000
    }
}

/// Leaves HSI as SYSCLK at 8 MHz (reset default, flash wait states 0).
pub fn init(_dp: &pac::Peripherals) -> Clocks {
    Clocks {
        sysclk: HSI_HZ,
        hclk: HSI_HZ,
        pclk1: HSI_HZ,
        pclk2: HSI_HZ,
    }
}
