//! I/D-Cache del Cortex-M7 — activar tras estabilizar el reloj del sistema.
//!
//! Usa las rutinas de `cortex-m` que incluyen invalidación y barreras DSB/ISB.

use cortex_m::peripheral::{CPUID, SCB};

/// Habilita I-cache y D-cache del M7 si aún están apagadas.
///
/// Debe llamarse después de [`crate::rcc::init`] cuando SYSCLK ya corre a
/// la frecuencia objetivo.
pub fn enable(scb: &mut SCB, cpuid: &mut CPUID) {
    scb.enable_icache();
    scb.enable_dcache(cpuid);
}
