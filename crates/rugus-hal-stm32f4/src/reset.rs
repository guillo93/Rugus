//! Causa del último reset (RCC_CSR) para STM32F4 — raw-MMIO.
//!
//! El bloque RCC registra en `CSR` la causa del reset más reciente con flags
//! pegajosos (sobreviven al reset, se limpian solo con `RMVF`). Leerlos al
//! arranque distingue un reset por watchdog (IWDG) de un power-on, un reset por
//! pin NRST o un reset por software (`SCB.SYSRESETREQ`). Es el complemento de la
//! telemetría persistente de faults (F4.4): el *por qué* del último arranque.
//!
//! Hay que limpiarlos (`RMVF`) tras leerlos, o el siguiente arranque heredaría
//! flags viejos. Acceso por MMIO directo, en la línea de [`crate::iwdg`].

use core::ptr::{read_volatile, write_volatile};

/// `RCC->CSR` (base RCC 0x4002_3800 + offset 0x74).
const RCC_CSR: u32 = 0x4002_3874;

// Flags de causa de reset (bits altos de CSR). Pegajosos hasta `RMVF`.
const LPWRRSTF: u32 = 1 << 31; // reset por entrada ilegal a Stop/Standby
const WWDGRSTF: u32 = 1 << 30; // window watchdog
const IWDGRSTF: u32 = 1 << 29; // independent watchdog
const SFTRSTF: u32 = 1 << 28; // software (SYSRESETREQ)
const PORRSTF: u32 = 1 << 27; // power-on / power-down
const PINRSTF: u32 = 1 << 26; // pin NRST
const BORRSTF: u32 = 1 << 25; // brown-out
const RMVF: u32 = 1 << 24; // remove flags (write-1 para limpiar)

/// Causa del último reset, decodificada desde `RCC_CSR`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResetCause {
    /// Power-on / power-down (arranque en frío).
    PowerOn,
    /// Brown-out (caída de tensión).
    Brownout,
    /// Pin externo NRST.
    Pin,
    /// Reset por software (`SCB.SYSRESETREQ`, p. ej. comando `reboot`).
    Software,
    /// Watchdog independiente (IWDG): el supervisor dejó de alimentarlo.
    IndependentWatchdog,
    /// Window watchdog (WWDG).
    WindowWatchdog,
    /// Entrada ilegal a modo de bajo consumo.
    LowPower,
    /// Ningún flag reconocido (no debería ocurrir tras un reset real).
    Unknown,
}

impl ResetCause {
    /// Nombre corto para logging sin `defmt`.
    pub fn name(self) -> &'static str {
        match self {
            ResetCause::PowerOn => "power-on",
            ResetCause::Brownout => "brownout",
            ResetCause::Pin => "pin-nrst",
            ResetCause::Software => "software",
            ResetCause::IndependentWatchdog => "iwdg",
            ResetCause::WindowWatchdog => "wwdg",
            ResetCause::LowPower => "low-power",
            ResetCause::Unknown => "unknown",
        }
    }
}

/// Lee la causa del último reset y limpia los flags (`RMVF`) para que el próximo
/// arranque parta de cero.
///
/// Cuando hay varios flags activos (p. ej. PIN + POR en un power-on real) se
/// prioriza la causa más informativa: watchdogs y software por encima del pin y
/// del power-on, que suelen acompañar a otras.
pub fn read_and_clear() -> ResetCause {
    // SAFETY: RCC_CSR es MMIO; lectura simple + write-1-to-clear de RMVF.
    let csr = unsafe { read_volatile(RCC_CSR as *const u32) };
    let cause = if csr & LPWRRSTF != 0 {
        ResetCause::LowPower
    } else if csr & WWDGRSTF != 0 {
        ResetCause::WindowWatchdog
    } else if csr & IWDGRSTF != 0 {
        ResetCause::IndependentWatchdog
    } else if csr & SFTRSTF != 0 {
        ResetCause::Software
    } else if csr & BORRSTF != 0 {
        ResetCause::Brownout
    } else if csr & PORRSTF != 0 {
        ResetCause::PowerOn
    } else if csr & PINRSTF != 0 {
        ResetCause::Pin
    } else {
        ResetCause::Unknown
    };
    // Limpia todos los flags para no heredarlos en el próximo arranque.
    unsafe { write_volatile(RCC_CSR as *mut u32, csr | RMVF) };
    cause
}
