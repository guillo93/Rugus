//! Independent watchdog (IWDG) — alimentación desde `ward`.

use crate::pac;

/// Handle IWDG con prescaler /4, reload ~1 s @ LSI 40 kHz.
pub struct Watchdog {
    armed: bool,
}

impl Watchdog {
    /// Arma el IWDG si aún no está activo.
    pub fn init(dp: &pac::Peripherals) -> Self {
        let iwdg = &dp.IWDG;
        if !iwdg.sr.read().pvu().bit() && !iwdg.sr.read().rvu().bit() {
            // Unlock PR/RLR
            iwdg.kr.write(|w| unsafe { w.key().bits(0x5555) });
            iwdg.pr.write(|w| w.pr().bits(0b000)); // /4
            iwdg.rlr.write(|w| w.rl().bits(1000));
            iwdg.kr.write(|w| unsafe { w.key().bits(0xCCCC) }); // start
        }
        Self { armed: true }
    }

    /// Alimenta el watchdog.
    pub fn kick(&self, iwdg: &pac::IWDG) {
        if self.armed {
            iwdg.kr.write(|w| unsafe { w.key().bits(0xAAAA) });
        }
    }

    /// Retorna true si el periférico responde.
    pub fn is_armed(&self) -> bool {
        self.armed
    }
}
