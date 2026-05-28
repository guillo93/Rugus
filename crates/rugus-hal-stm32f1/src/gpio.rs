//! GPIO mínimo para STM32F103 Blue Pill — LED en PC13 (activo en bajo).
//!
//! API typestate completa llegará cuando un ejemplo lo exija. Por ahora,
//! helper directo para el LED onboard.

use crate::pac;
use rugus_hal::GpioPin;

/// On-board user LED on generic Blue Pill clones (PC13, active **low**).
#[derive(Clone, Copy, Debug)]
pub enum BluePillLed {
    /// PC13 — inverted: `set_low` turns the LED on.
    Pc13,
}

/// Handle for an LED configured as push-pull output.
pub struct LedPin {
    led: BluePillLed,
}

impl LedPin {
    /// Creates the handle and configures the pin as push-pull output.
    pub fn new(rcc: &pac::RCC, led: BluePillLed) -> Self {
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
        // SAFETY: read-modify-write on ODR single bit; blink is single-threaded.
        unsafe {
            let g = &*pac::GPIOC::ptr();
            let bit = pin_bit(self.led);
            g.odr.modify(|r, w| w.bits(r.bits() ^ bit));
        }
        Ok(())
    }

    fn is_high(&self) -> Result<bool, Self::Error> {
        let level = unsafe {
            let g = &*pac::GPIOC::ptr();
            g.idr.read().bits() & pin_bit(self.led) != 0
        };
        Ok(level)
    }
}

fn pin_bit(led: BluePillLed) -> u32 {
    1 << pin_number(led)
}

fn pin_number(led: BluePillLed) -> u8 {
    match led {
        BluePillLed::Pc13 => 13,
    }
}

fn enable_clock(rcc: &pac::RCC) {
    rcc.apb2enr.modify(|_, w| w.iopcen().set_bit());
    let _ = rcc.apb2enr.read().bits();
}

fn configure_output(led: BluePillLed) {
    let pin = pin_number(led);
    let shift = (pin - 8) as u32 * 4;
    // Output push-pull, max 2 MHz (CNF=00, MODE=10).
    let nibble = 0b10u32;
    // SAFETY: we only touch CRH bits for the owned pin on GPIOC.
    unsafe {
        let g = &*pac::GPIOC::ptr();
        g.crh
            .modify(|r, w| w.bits((r.bits() & !(0b1111 << shift)) | (nibble << shift)));
    }
}

fn write_bsrr(led: BluePillLed, high: bool) {
    let pin = pin_number(led);
    // SAFETY: BSRR is write-only and atomic per bit.
    unsafe {
        let g = &*pac::GPIOC::ptr();
        g.bsrr
            .write(|w| w.bits(if high { 1 << pin } else { 1 << (pin + 16) }));
    }
}
