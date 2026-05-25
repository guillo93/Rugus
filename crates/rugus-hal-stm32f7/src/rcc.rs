//! RCC — reloj del sistema para STM32F769I-DISCO.
//!
//! Configura HSE 25 MHz (PH0/PH1) → PLL → SYSCLK 216 MHz con divisores
//! AHB/APB y latencia flash según RM0385. Requiere over-drive en VOS scale 1.

use crate::pac;

/// Frecuencia del cristal HSE en la STM32F769I-DISCO (UM2033).
pub const HSE_HZ: u32 = 25_000_000;

/// SYSCLK objetivo para F769 @ VOS scale 1 + over-drive.
pub const SYSCLK_HZ: u32 = 216_000_000;

/// Relojes del bus tras `init`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    /// Frecuencia del núcleo (SYSCLK / HPRE).
    pub sysclk: u32,
    /// HCLK = SYSCLK con HPRE /1.
    pub hclk: u32,
    /// PCLK1 (APB1), máx. 54 MHz.
    pub pclk1: u32,
    /// PCLK2 (APB2), máx. 108 MHz.
    pub pclk2: u32,
}

impl Clocks {
    /// SYSCLK en megahertz, útil para logs `defmt`.
    pub const fn sysclk_mhz(&self) -> u32 {
        self.sysclk / 1_000_000
    }
}

/// Inicializa el árbol de relojes a 216 MHz desde HSE 25 MHz.
///
/// Secuencia: PWR → VOS scale 1 → over-drive → flash WS7 + ART → HSE → PLL →
/// divisores AHB/APB → switch a PLL. Llama a [`crate::cache::enable`] después
/// si se desea I/D-cache (requiere `cortex_m::Peripherals`).
pub fn init(dp: &pac::Peripherals) -> Clocks {
    let rcc = &dp.RCC;
    let pwr = &dp.PWR;
    let flash = &dp.FLASH;

    enable_pwr_clock(rcc);
    configure_voltage_scale(pwr);
    enable_overdrive(pwr);
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

fn enable_pwr_clock(rcc: &pac::RCC) {
    rcc.apb1enr.modify(|_, w| w.pwren().enabled());
    // Lectura dummy para que el enable se propague (RM0385 §6.3.14).
    let _ = rcc.apb1enr.read().bits();
}

fn configure_voltage_scale(pwr: &pac::PWR) {
    // Tras reset, CR1.VOS ya es Scale 1 (0xC000) pero CSR1.VOSRDY puede seguir
    // en 0 hasta que haya una transición real de VOS. Re-escribir Scale 1 no
    // desbloquea VOSRDY y provoca hang; solo programar y esperar si cambiamos VOS.
    if !pwr.cr1.read().vos().is_scale1() {
        pwr.cr1.modify(|_, w| w.vos().scale1());
        wait_until(|| pwr.csr1.read().vosrdy().bit());
    }
}

fn enable_overdrive(pwr: &pac::PWR) {
    pwr.cr1.modify(|_, w| w.oden().set_bit());
    wait_until(|| pwr.csr1.read().odrdy().bit());

    pwr.cr1.modify(|_, w| w.odswen().set_bit());
    wait_until(|| pwr.csr1.read().odswrdy().bit());
}

fn configure_flash(flash: &pac::FLASH) {
    flash.acr.modify(|_, w| {
        w.latency().ws7();
        w.prften().enabled();
        w.arten().enabled()
    });
}

fn enable_hse(rcc: &pac::RCC) {
    rcc.cr.modify(|_, w| w.hseon().set_bit());
    wait_until(|| rcc.cr.read().hserdy().bit());
}

fn configure_pll(rcc: &pac::RCC) {
    // VCO_in = 25 MHz / 25 = 1 MHz; VCO = 432 MHz; SYSCLK = 432 / 2 = 216 MHz.
    rcc.pllcfgr.modify(|_, w| {
        w.pllsrc().hse();
        // PLLM/PLLN/PLLQ values are validated against RM0385 constraints above.
        unsafe {
            w.pllm().bits(25);
            w.plln().bits(432);
            w.pllq().bits(9);
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
