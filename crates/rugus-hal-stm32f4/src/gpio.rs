//! GPIO mínimo para STM32F4 — LEDs de la STM32F407G-DISC1 (UM1472 §6.4).
//!
//! API typestate completa llegará cuando un ejemplo lo exija. Por ahora,
//! helpers directos para LD3–LD6 en PD12–PD15.

use crate::pac;
use rugus_hal::GpioPin;

/// User LEDs on STM32F407G-DISC1 (all on GPIOD, active high).
#[derive(Clone, Copy, Debug)]
pub enum DiscoLed {
    /// LD4, green, PD12.
    Green,
    /// LD3, orange, PD13.
    Orange,
    /// LD5, red, PD14.
    Red,
    /// LD6, blue, PD15.
    Blue,
}

/// Handle for an LED configured as push-pull output.
pub struct LedPin {
    led: DiscoLed,
}

impl LedPin {
    /// Creates the handle and configures the pin as push-pull output.
    pub fn new(rcc: &pac::RCC, led: DiscoLed) -> Self {
        enable_clock(rcc);
        configure_output(led);
        Self { led }
    }
}

impl GpioPin for LedPin {
    type Error = core::convert::Infallible;

    fn set_high(&mut self) -> Result<(), Self::Error> {
        write_bsrr(self.led, true);
        Ok(())
    }

    fn set_low(&mut self) -> Result<(), Self::Error> {
        write_bsrr(self.led, false);
        Ok(())
    }

    fn toggle(&mut self) -> Result<(), Self::Error> {
        // SAFETY: read-modify-write on ODR single bit; G3 blink is single-threaded.
        unsafe {
            let g = &*pac::GPIOD::ptr();
            let bit = pin_bit(self.led);
            g.odr.modify(|r, w| w.bits(r.bits() ^ bit));
        }
        Ok(())
    }

    fn is_high(&self) -> Result<bool, Self::Error> {
        let level = unsafe {
            let g = &*pac::GPIOD::ptr();
            g.idr.read().bits() & pin_bit(self.led) != 0
        };
        Ok(level)
    }
}

fn pin_bit(led: DiscoLed) -> u32 {
    1 << pin_number(led)
}

fn pin_number(led: DiscoLed) -> u8 {
    match led {
        DiscoLed::Green => 12,
        DiscoLed::Orange => 13,
        DiscoLed::Red => 14,
        DiscoLed::Blue => 15,
    }
}

fn enable_clock(rcc: &pac::RCC) {
    rcc.ahb1enr.modify(|_, w| w.gpioden().set_bit());
    let _ = rcc.ahb1enr.read().bits();
}

fn configure_output(led: DiscoLed) {
    let pin = pin_number(led);
    let shift = pin as u32 * 2;
    // SAFETY: we only touch MODER/OTYPER bits for the owned pin on GPIOD.
    unsafe {
        let g = &*pac::GPIOD::ptr();
        g.moder
            .modify(|r, w| w.bits((r.bits() & !(0b11 << shift)) | (0b01 << shift)));
        g.otyper.modify(|r, w| w.bits(r.bits() & !(1 << pin)));
    }
}

fn write_bsrr(led: DiscoLed, high: bool) {
    let pin = pin_number(led);
    // SAFETY: BSRR is write-only and atomic per bit.
    unsafe {
        let g = &*pac::GPIOD::ptr();
        g.bsrr
            .write(|w| w.bits(if high { 1 << pin } else { 1 << (pin + 16) }));
    }
}
