//! GPIO mínimo para STM32F7 — suficiente para el ejemplo blink.
//!
//! API completa con typestate (modo input/output/alternate función)
//! llegará en G2. Por ahora, helpers directos para los 4 LEDs de la
//! STM32F769I-DISCO.

use crate::pac;
use rugus_hal::GpioPin;

/// Los 4 LEDs de usuario de la STM32F769I-DISCO (UM2033 §6.5).
#[derive(Clone, Copy, Debug)]
pub enum DiscoLed {
    /// LD1, rojo, PJ13.
    Red,
    /// LD2, verde, PJ5.
    Green,
    /// LD3, rojo, PA12.
    Red2,
    /// LD4, verde, PD4.
    Green2,
}

/// Handle de un LED inicializado como salida push-pull.
pub struct LedPin {
    led: DiscoLed,
}

impl LedPin {
    /// Crea el handle y configura el pin como salida push-pull.
    ///
    /// Idempotente: llamar dos veces con el mismo LED no causa daño pero
    /// duplica el coste.
    pub fn new(rcc: &pac::RCC, led: DiscoLed) -> Self {
        enable_clock(rcc, led);
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
        // SAFETY: read-modify-write de ODR de un solo bit; en G0 no hay
        // concurrencia. En G2+ migrar a BSRR exclusivamente para evitar
        // la ventana RMW.
        unsafe {
            match self.led {
                DiscoLed::Red => {
                    let g = &*pac::GPIOJ::ptr();
                    g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 13)));
                }
                DiscoLed::Green => {
                    let g = &*pac::GPIOJ::ptr();
                    g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 5)));
                }
                DiscoLed::Red2 => {
                    let g = &*pac::GPIOA::ptr();
                    g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 12)));
                }
                DiscoLed::Green2 => {
                    let g = &*pac::GPIOD::ptr();
                    g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 4)));
                }
            }
        }
        Ok(())
    }

    fn is_high(&self) -> Result<bool, Self::Error> {
        // SAFETY: lectura atómica de IDR.
        let level = unsafe {
            match self.led {
                DiscoLed::Red => (*pac::GPIOJ::ptr()).idr.read().bits() & (1 << 13) != 0,
                DiscoLed::Green => (*pac::GPIOJ::ptr()).idr.read().bits() & (1 << 5) != 0,
                DiscoLed::Red2 => (*pac::GPIOA::ptr()).idr.read().bits() & (1 << 12) != 0,
                DiscoLed::Green2 => (*pac::GPIOD::ptr()).idr.read().bits() & (1 << 4) != 0,
            }
        };
        Ok(level)
    }
}

fn enable_clock(rcc: &pac::RCC, led: DiscoLed) {
    match led {
        DiscoLed::Red | DiscoLed::Green => {
            rcc.ahb1enr.modify(|_, w| w.gpiojen().enabled());
        }
        DiscoLed::Red2 => {
            rcc.ahb1enr.modify(|_, w| w.gpioaen().enabled());
        }
        DiscoLed::Green2 => {
            rcc.ahb1enr.modify(|_, w| w.gpioden().enabled());
        }
    }
}

fn configure_output(led: DiscoLed) {
    // SAFETY: configuramos solo los bits del pin que poseemos. MODER y
    // OTYPER no son tocados por nadie más en G0 fuera de este módulo.
    unsafe {
        match led {
            DiscoLed::Red => {
                let g = &*pac::GPIOJ::ptr();
                g.moder.modify(|r, w| w.bits((r.bits() & !(0b11 << 26)) | (0b01 << 26)));
                g.otyper.modify(|r, w| w.bits(r.bits() & !(1 << 13)));
            }
            DiscoLed::Green => {
                let g = &*pac::GPIOJ::ptr();
                g.moder.modify(|r, w| w.bits((r.bits() & !(0b11 << 10)) | (0b01 << 10)));
                g.otyper.modify(|r, w| w.bits(r.bits() & !(1 << 5)));
            }
            DiscoLed::Red2 => {
                let g = &*pac::GPIOA::ptr();
                g.moder.modify(|r, w| w.bits((r.bits() & !(0b11 << 24)) | (0b01 << 24)));
                g.otyper.modify(|r, w| w.bits(r.bits() & !(1 << 12)));
            }
            DiscoLed::Green2 => {
                let g = &*pac::GPIOD::ptr();
                g.moder.modify(|r, w| w.bits((r.bits() & !(0b11 << 8)) | (0b01 << 8)));
                g.otyper.modify(|r, w| w.bits(r.bits() & !(1 << 4)));
            }
        }
    }
}

fn write_bsrr(led: DiscoLed, high: bool) {
    // SAFETY: BSRR es write-only y atómico por bit (set/reset en mismo word).
    unsafe {
        match led {
            DiscoLed::Red => {
                let g = &*pac::GPIOJ::ptr();
                g.bsrr.write(|w| w.bits(if high { 1 << 13 } else { 1 << (13 + 16) }));
            }
            DiscoLed::Green => {
                let g = &*pac::GPIOJ::ptr();
                g.bsrr.write(|w| w.bits(if high { 1 << 5 } else { 1 << (5 + 16) }));
            }
            DiscoLed::Red2 => {
                let g = &*pac::GPIOA::ptr();
                g.bsrr.write(|w| w.bits(if high { 1 << 12 } else { 1 << (12 + 16) }));
            }
            DiscoLed::Green2 => {
                let g = &*pac::GPIOD::ptr();
                g.bsrr.write(|w| w.bits(if high { 1 << 4 } else { 1 << (4 + 16) }));
            }
        }
    }
}
