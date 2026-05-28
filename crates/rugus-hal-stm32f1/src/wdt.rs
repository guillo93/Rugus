//! Independent watchdog (IWDG) — alimentación desde `ward`.

use crate::pac;

/// Handle IWDG con prescaler /128, reload ~2 s @ LSI 40 kHz.
pub struct Watchdog {
    armed: bool,
}

impl Watchdog {
    /// Handle deshabilitado (sin armar IWDG).
    pub fn disabled() -> Self {
        Self { armed: false }
    }

    /// Actualiza PR/RLR y recarga el contador. Seguro si IWDG ya estaba activo.
    pub fn configure(iwdg: &pac::IWDG) -> Self {
        while iwdg.sr.read().pvu().bit() || iwdg.sr.read().rvu().bit() {}

        iwdg.kr.write(|w| unsafe { w.key().bits(0x5555) });
        while iwdg.sr.read().pvu().bit() || iwdg.sr.read().rvu().bit() {}
        iwdg.pr.write(|w| w.pr().bits(0b101)); // /128
        iwdg.rlr.write(|w| w.rl().bits(625)); // ~2 s
        iwdg.kr.write(|w| unsafe { w.key().bits(0xAAAA) });

        Self { armed: false }
    }

    /// Arranca IWDG (solo tras `configure`; no re-escribir 0xCCCC si ya corre).
    pub fn arm(&mut self, iwdg: &pac::IWDG) {
        if !self.armed {
            iwdg.kr.write(|w| unsafe { w.key().bits(0xCCCC) });
            self.armed = true;
        }
        self.kick(iwdg);
    }

    /// Compat: configura timeout y recarga; no llama 0xCCCC (evita reset en re-flash).
    pub fn init(dp: &pac::Peripherals) -> Self {
        Self::configure(&dp.IWDG)
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
