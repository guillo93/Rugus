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
const WINR: u32 = 0x10;

// Llaves del key register.
const KEY_RELOAD: u32 = 0xAAAA;
const KEY_ENABLE_WRITE: u32 = 0x5555;
const KEY_START: u32 = 0xCCCC;

/// Prescaler /128 (PR=0b101): con LSI ~32 kHz da ~250 Hz (tick ~4 ms).
const PR_DIV128: u32 = 0b101;
/// Reload ~2 s: 250 Hz * 2 s = 500 ticks. Holgado frente al kick del supervisor.
const RLR_2S: u32 = 500;
/// Ventana al ~50 % (250 ticks ≈ 1 s). Con modo windowed, alimentar ANTES de que
/// el contador baje de este valor (es decir, menos de ~1 s tras la última
/// recarga) dispara un reset: detecta un supervisor que itera descontrolado
/// (demasiado rápido), no solo uno colgado. El kick debe caer en [~1 s, ~2 s].
const WINR_HALF: u32 = 250;

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

    /// Como [`Self::start`] pero en **modo windowed**: además del límite superior
    /// (~2 s sin kick → reset por cuelgue), fija una ventana inferior ([`WINR_HALF`])
    /// de modo que alimentar demasiado pronto (< ~1 s tras la recarga) también
    /// resetea. Detecta un supervisor en bucle desbocado, no solo uno parado.
    ///
    /// El supervisor debe espaciar su kick para caer en [~1 s, ~2 s]; ver el
    /// ejemplo de placa. Escribir WINR recarga el contador automáticamente.
    pub fn start_windowed() -> Self {
        // SAFETY: registros MMIO del IWDG; arranque single-thread. Igual que
        // `start`, pero programa WINR al final (tras KEY_START enciende el LSI):
        // el write de WINR exige rehabilitar escritura (KEY_ENABLE_WRITE) y, por
        // hardware, recarga el contador (no hace falta KEY_RELOAD adicional).
        unsafe {
            write_reg(KR, KEY_ENABLE_WRITE);
            write_reg(PR, PR_DIV128);
            write_reg(RLR, RLR_2S);
            write_reg(KR, KEY_START);
            write_reg(KR, KEY_ENABLE_WRITE);
            write_reg(WINR, WINR_HALF);
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
