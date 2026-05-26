//! RCC — system clock for STM32F407G-DISC1.
//!
//! Configures HSE 8 MHz (PH0/PH1) → PLL → SYSCLK 168 MHz with AHB/APB
//! prescalers and flash latency per RM0090.

use crate::pac;

/// HSE crystal frequency on STM32F407G-DISC1 (UM1472).
pub const HSE_HZ: u32 = 8_000_000;

/// Target SYSCLK for F407 (device maximum).
pub const SYSCLK_HZ: u32 = 168_000_000;

/// Bus clocks after [`init`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    /// Core frequency (SYSCLK / HPRE).
    pub sysclk: u32,
    /// HCLK.
    pub hclk: u32,
    /// PCLK1 (APB1), max 42 MHz.
    pub pclk1: u32,
    /// PCLK2 (APB2), max 84 MHz.
    pub pclk2: u32,
}

impl Clocks {
    /// SYSCLK in megahertz for `defmt` logs.
    pub const fn sysclk_mhz(&self) -> u32 {
        self.sysclk / 1_000_000
    }
}

/// Initializes the clock tree to 168 MHz from HSE 8 MHz.
pub fn init(dp: &pac::Peripherals) -> Clocks {
    let rcc = &dp.RCC;
    let flash = &dp.FLASH;

    configure_flash(flash);
    enable_hse(rcc);
    configure_pll(rcc);
    enable_pll(rcc);
    configure_bus_prescalers(rcc);
    switch_to_pll(rcc);

    Clocks {
        sysclk: SYSCLK_HZ,
        hclk: SYSCLK_HZ,
        pclk1: SYSCLK_HZ / 4,
        pclk2: SYSCLK_HZ / 2,
    }
}

fn configure_flash(flash: &pac::FLASH) {
    flash.acr.modify(|_, w| {
        w.latency().ws5();
        w.prften().enabled()
    });
}

fn enable_hse(rcc: &pac::RCC) {
    rcc.cr.modify(|_, w| w.hseon().set_bit());
    wait_until(|| rcc.cr.read().hserdy().bit());
}

fn configure_pll(rcc: &pac::RCC) {
    // VCO_in = 8 MHz / 8 = 1 MHz; VCO = 336 MHz; SYSCLK = 336 / 2 = 168 MHz.
    rcc.pllcfgr.modify(|_, w| {
        w.pllsrc().hse();
        // PLLM/PLLN/PLLQ validated against RM0090 constraints for 168 MHz SYSCLK.
        unsafe {
            w.pllm().bits(8);
            w.plln().bits(336);
            w.pllq().bits(7);
        }
        w.pllp().div2()
    });
}

fn enable_pll(rcc: &pac::RCC) {
    rcc.cr.modify(|_, w| w.pllon().set_bit());
    wait_until(|| rcc.cr.read().pllrdy().bit());
}

fn configure_bus_prescalers(rcc: &pac::RCC) {
    rcc.cfgr.modify(|_, w| {
        w.hpre().div1();
        w.ppre1().div4();
        w.ppre2().div2()
    });
}

fn switch_to_pll(rcc: &pac::RCC) {
    rcc.cfgr.modify(|_, w| w.sw().pll());
    wait_until(|| rcc.cfgr.read().sws().is_pll());
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    while !predicate() {}
}
