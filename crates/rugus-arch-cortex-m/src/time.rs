//! Reloj monotónico por SysTick (específico de Cortex-M).
//!
//! El `Arch` trait se mantiene mínimo y agnóstico de periféricos; el tiempo
//! es una capacidad propia del backend Cortex-M, igual que la MPU. Un tick de
//! SysTick a 1 kHz incrementa un contador de milisegundos en
//! [`now_ms`](crate::time::now_ms). La plataforma decide la frecuencia del core
//! ([`init`]) y construye sus primitivas de espera cooperativa sobre `now_ms`.
//!
//! `u32` de milisegundos desborda a ~49,7 días de uptime continuo; suficiente
//! para un appliance lite y barato (no hay atómicos de 64 bits en el M3).

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::peripheral::syst::SystClkSource;
use cortex_m::peripheral::SYST;

/// Milisegundos desde [`init`]. Productor único: ISR SysTick.
static MILLIS: AtomicU32 = AtomicU32::new(0);

/// ISR de SysTick: +1 ms por tick.
///
/// Tras la feature `systick` (default-on): un binario que aporte su propio
/// handler de SysTick desactiva esta feature para evitar el doble símbolo.
#[cfg(feature = "systick")]
#[cortex_m_rt::exception]
fn SysTick() {
    MILLIS.fetch_add(1, Ordering::Relaxed);
}

/// Arranca SysTick a 1 kHz (tick de 1 ms) usando el reloj del core.
///
/// `core_hz` es la frecuencia del HCLK/core (p. ej. 8_000_000 en el F103 con
/// HSI sin PLL). El reload es de 24 bits, así que `core_hz/1000` debe caber en
/// 0x00FF_FFFF (hasta ~16 GHz). Habilita el contador y la interrupción.
pub fn init(syst: &mut SYST, core_hz: u32) {
    let reload = (core_hz / 1000).saturating_sub(1) & 0x00FF_FFFF;
    syst.set_clock_source(SystClkSource::Core);
    syst.set_reload(reload);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();
}

/// Milisegundos transcurridos desde [`init`] (monotónico, envuelve a ~49 días).
#[inline]
pub fn now_ms() -> u32 {
    MILLIS.load(Ordering::Relaxed)
}

/// Milisegundos transcurridos desde `since` con aritmética envolvente.
#[inline]
pub fn elapsed_ms(since: u32) -> u32 {
    now_ms().wrapping_sub(since)
}
