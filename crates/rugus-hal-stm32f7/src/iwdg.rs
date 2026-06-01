//! Independent watchdog (IWDG) para STM32F7 — raw-MMIO, gemelo del de F4.
//!
//! El IWDG corre de un reloj LSI propio (~32 kHz), independiente del SYSCLK: si
//! el kernel se cuelga y deja de alimentarlo, dispara un reset de sistema. Es la
//! última red de seguridad del failsafe (el WFI terminal, con todas las tareas
//! muertas, deja de alimentarlo → reset → arranque limpio).
//!
//! El bloque IWDG es idéntico en F4/F7 (mismos offsets KR/PR/RLR/SR en
//! 0x4000_3000), por eso este driver es un gemelo exacto del de F4. Acceso por
//! MMIO directo en la misma línea que [`crate::gpio`]/[`crate::usart`].
//!
//! Secuencia: `start` desbloquea (KR=0x5555), fija prescaler y reload, luego lo
//! arranca (KR=0xCCCC) — esto también enciende el LSI automáticamente. A partir
//! de ahí hay que `kick` (KR=0xAAAA) antes de que venza el reload.

use core::ptr::write_volatile;

/// Base del IWDG (idéntica en F4/F7).
const IWDG_BASE: u32 = 0x4000_3000;

// Offsets de registro.
const KR: u32 = 0x00;
const PR: u32 = 0x04;
const RLR: u32 = 0x08;

// Llaves del key register.
const KEY_RELOAD: u32 = 0xAAAA;
const KEY_ENABLE_WRITE: u32 = 0x5555;
const KEY_START: u32 = 0xCCCC;

/// Prescaler /128 (PR=0b101): con LSI ~32 kHz da ~250 Hz (tick ~4 ms).
const PR_DIV128: u32 = 0b101;
/// Reload ~2 s: 250 Hz * 2 s = 500 ticks. Holgado frente al kick del supervisor.
const RLR_2S: u32 = 500;

/// Handle del watchdog independiente.
pub struct Iwdg {
    armed: bool,
}

impl Iwdg {
    /// Configura prescaler /128 y reload ~2 s, y arranca el IWDG. Tras esto hay
    /// que [`Self::kick`] periódicamente o el chip se resetea.
    pub fn start() -> Self {
        // SAFETY: registros MMIO del IWDG; arranque single-thread.
        //
        // No se sondea SR.PVU/RVU: esos flags solo se actualizan con el LSI en
        // marcha, y el LSI no arranca hasta KEY_START (0xCCCC). Sondearlos antes
        // de arrancar cuelga (LSI parado → nunca se limpian). Habilitamos
        // escritura, programamos PR/RLR, arrancamos (esto enciende el LSI) y
        // recargamos; el hardware aplica PR/RLR antes del primer timeout.
        unsafe {
            write_reg(KR, KEY_ENABLE_WRITE);
            write_reg(PR, PR_DIV128);
            write_reg(RLR, RLR_2S);
            write_reg(KR, KEY_START);
            write_reg(KR, KEY_RELOAD);
        }
        Self { armed: true }
    }

    /// Alimenta el watchdog (recarga el contador). No-op si no está armado.
    pub fn kick(&self) {
        if self.armed {
            // SAFETY: escribir la llave de reload es atómico y siempre seguro.
            unsafe { write_reg(KR, KEY_RELOAD) }
        }
    }
}

#[inline]
unsafe fn write_reg(off: u32, val: u32) {
    unsafe { write_volatile((IWDG_BASE + off) as *mut u32, val) }
}
