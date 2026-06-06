//! Reloj monotónico por SysTick (específico de Cortex-M).
//!
//! El `Arch` trait se mantiene mínimo y agnóstico de periféricos; el tiempo
//! es una capacidad propia del backend Cortex-M, igual que la MPU. Un tick de
//! SysTick a 1 kHz incrementa un contador de milisegundos en
//! [`now_ms`]. La plataforma decide la frecuencia del core
//! ([`init`]) y construye sus primitivas de espera cooperativa sobre `now_ms`.
//!
//! `u32` de milisegundos desborda a ~49,7 días de uptime continuo; suficiente
//! para un appliance lite y barato (no hay atómicos de 64 bits en el M3).
//!
//! # Tick dinámico (feature `tickless`, F5.A.1)
//!
//! Con `tickless`, el reloj deja de interrumpir cada milisegundo cuando el
//! núcleo está ocioso: la capa de scheduler invoca `idle_until` con los
//! milisegundos hasta el próximo plazo y aquí se reprograma el reload de SysTick
//! a ese intervalo (acotado por el límite de 24 bits del temporizador). La ISR
//! sigue siendo el único productor de `MILLIS`: cuando el intervalo extendido
//! expira, suma de golpe los milisegundos que representaba y restaura el tick de
//! 1 ms. Si una IRQ externa despierta el `wfi` ANTES de que expire, `idle_until`
//! contabiliza el tiempo parcial leyendo el contador y restaura el tick de 1 ms,
//! coordinándose con la ISR vía `TICK_MS` y el flag COUNTFLAG para no perder ni
//! duplicar tiempo (el resto sub-milisegundo se acarrea en `REMAINDER`).

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use cortex_m::peripheral::syst::SystClkSource;
use cortex_m::peripheral::SYST;

/// Milisegundos desde [`init`]. Productor único: ISR SysTick.
static MILLIS: AtomicU32 = AtomicU32::new(0);

/// Nº de veces que la ISR de SysTick se ha disparado desde [`init`].
/// Observable por [`systick_irqs`]: con tick fijo crece ~1000/s; con tick
/// dinámico (`tickless`) crece mucho menos en idle (evidencia del ahorro).
static SYSTICK_IRQS: AtomicU32 = AtomicU32::new(0);

/// Cuentas de SysTick por milisegundo (= `core_hz / 1000`), fijado en [`init`].
/// Base de la conversión cuentas↔ms del tick dinámico. `1` como mínimo seguro.
static UNITS_PER_MS: AtomicU32 = AtomicU32::new(1);

/// Máximo de milisegundos que cabe en un único intervalo de SysTick (reload de
/// 24 bits): `(0x0100_0000) / UNITS_PER_MS`. Fijado en [`init`]; acota cuánto
/// puede dormir el tick dinámico de una sola vez.
static MAX_TICK_MS: AtomicU32 = AtomicU32::new(1);

/// Milisegundos que representa el intervalo de SysTick actualmente programado.
/// `1` en operación normal; un valor mayor mientras un idle extendido está en
/// vuelo. La ISR lo suma a `MILLIS` en cada expiración y lo devuelve a `1`.
static TICK_MS: AtomicU32 = AtomicU32::new(1);

/// Acarreo de cuentas sub-milisegundo perdidas al contabilizar despertares
/// parciales (IRQ externa antes de que expire el intervalo extendido). Evita la
/// deriva acumulada del redondeo a la baja.
static REMAINDER: AtomicU32 = AtomicU32::new(0);

/// Puntero a la función de preempción que la capa de kernel registra con
/// [`set_preempt_hook`]; 0 = sin hook (clock monotónico puro, sin preempción).
/// La ISR de SysTick la invoca en cada tick. Se guarda como `usize` porque no
/// hay atómico de punteros a `fn` portable; el cast es de ida y vuelta exacto.
static PREEMPT_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Dirección del registro ICSR del SCB (bit 25 = PENDSTCLR: limpia un SysTick
/// pendiente). Se escribe directo para no robar el periférico SCB en el camino
/// de idle.
#[cfg(feature = "tickless")]
const SCB_ICSR: *mut u32 = 0xE000_ED04 as *mut u32;
/// PENDSTCLR: escribir 1 limpia el SysTick pendiente.
#[cfg(feature = "tickless")]
const ICSR_PENDSTCLR: u32 = 1 << 25;

/// Registra la función de preempción que la ISR de SysTick llamará cada tick.
///
/// La capa de kernel pasa aquí un trampolín que rutea a su scheduler
/// (`preempt_tick`). Sin hook registrado, SysTick solo lleva el reloj.
pub fn set_preempt_hook(hook: fn()) {
    PREEMPT_HOOK.store(hook as usize, Ordering::Relaxed);
}

/// Dispara el hook de preempción si hay alguno registrado.
#[cfg(feature = "systick")]
#[inline(always)]
fn fire_preempt_hook() {
    let hook = PREEMPT_HOOK.load(Ordering::Relaxed);
    if hook != 0 {
        // SAFETY: solo se escribe en `set_preempt_hook` con un `fn()` válido;
        // el cast usize→fn() revierte exactamente el store.
        let f: fn() = unsafe { core::mem::transmute(hook) };
        f();
    }
}

/// ISR de SysTick: avanza el reloj y, si hay hook, dispara la preempción.
///
/// Sin `tickless`: +1 ms por tick (1 kHz fijo). Con `tickless`: suma los ms que
/// representa el intervalo expirado (`TICK_MS`) y, si era extendido, restaura
/// el tick de 1 ms para volver a operación normal.
///
/// Tras la feature `systick` (default-on): un binario que aporte su propio
/// handler de SysTick desactiva esta feature para evitar el doble símbolo.
#[cfg(all(feature = "systick", not(feature = "tickless")))]
#[cortex_m_rt::exception]
fn SysTick() {
    SYSTICK_IRQS.fetch_add(1, Ordering::Relaxed);
    MILLIS.fetch_add(1, Ordering::Relaxed);
    fire_preempt_hook();
}

#[cfg(all(feature = "systick", feature = "tickless"))]
#[cortex_m_rt::exception]
fn SysTick() {
    // Reclama el intervalo expirado de forma atómica frente a `idle_until`
    // (que también puede ponerlo a 1 en un despertar parcial): el que gane el
    // swap es el que contabiliza.
    SYSTICK_IRQS.fetch_add(1, Ordering::Relaxed);
    let tick = TICK_MS.swap(1, Ordering::Relaxed);
    MILLIS.fetch_add(tick, Ordering::Relaxed);
    if tick != 1 {
        // El intervalo era extendido: restaura el reload de 1 ms para los
        // próximos ticks (el contador ya recargó al valor extendido al envolver;
        // reprogramar + limpiar el contador reanuda a 1 kHz).
        let units = UNITS_PER_MS.load(Ordering::Relaxed);
        // SAFETY: SysTick es de uso exclusivo de este módulo de tiempo; en la
        // ISR no concurre con `idle_until` (corre en modo hilo con IRQs activas
        // solo fuera de su sección crítica).
        let mut syst = unsafe { cortex_m::Peripherals::steal().SYST };
        syst.set_reload(units.saturating_sub(1) & 0x00FF_FFFF);
        syst.clear_current();
    }
    fire_preempt_hook();
}

/// Arranca SysTick a 1 kHz (tick de 1 ms) usando el reloj del core.
///
/// `core_hz` es la frecuencia del HCLK/core (p. ej. 8_000_000 en el F103 con
/// HSI sin PLL). El reload es de 24 bits, así que `core_hz/1000` debe caber en
/// 0x00FF_FFFF (hasta ~16 GHz). Habilita el contador y la interrupción.
pub fn init(syst: &mut SYST, core_hz: u32) {
    let units = (core_hz / 1000).max(1);
    UNITS_PER_MS.store(units, Ordering::Relaxed);
    // Máximo de ms en un reload de 24 bits: (2^24) cuentas / cuentas-por-ms.
    MAX_TICK_MS.store((0x0100_0000u32 / units).max(1), Ordering::Relaxed);
    TICK_MS.store(1, Ordering::Relaxed);
    REMAINDER.store(0, Ordering::Relaxed);
    let reload = units.saturating_sub(1) & 0x00FF_FFFF;
    syst.set_clock_source(SystClkSource::Core);
    syst.set_reload(reload);
    syst.clear_current();
    // SysTick a la MISMA prioridad que PendSV (0xFF, la más baja). Cuando un
    // cambio cooperativo deja PendSV pendiente y un tick de SysTick coincide al
    // desenmascarar, el empate lo rompe el núm. de excepción: PendSV (14) gana a
    // SysTick (15), así que el switch cooperativo se completa ANTES de que la
    // preempción observe un `current` ya actualizado pero aún sin conmutar.
    // SAFETY: registro SHPR del SCB; configuración de arranque single-thread.
    unsafe {
        use cortex_m::peripheral::scb::SystemHandler;
        let mut scb = cortex_m::Peripherals::steal().SCB;
        scb.set_priority(SystemHandler::SysTick, 0xFF);
    }
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

/// Nº de disparos de la ISR de SysTick desde [`init`]. Métrica de despertares:
/// comparar contra [`now_ms`] mide cuánto reduce el tick dinámico las
/// interrupciones en idle (con tick fijo, irqs ≈ ms).
#[inline]
pub fn systick_irqs() -> u32 {
    SYSTICK_IRQS.load(Ordering::Relaxed)
}

/// Espera ociosa con tick dinámico (F5.A.1): duerme el núcleo hasta el próximo
/// plazo del scheduler (`next_wake_ms`) o hasta una IRQ externa, reprogramando
/// SysTick para no interrumpir cada milisegundo.
///
/// `next_wake_ms`: ms hasta el próximo despertar por reloj (saturado a 0 si ya
/// venció), o `None` si solo una IRQ externa puede reanudar. Tras esta llamada
/// el reloj queda re-sincronizado y el tick vuelve a 1 ms.
#[cfg(feature = "tickless")]
pub fn idle_until(next_wake_ms: Option<u32>) {
    let units = UNITS_PER_MS.load(Ordering::Relaxed);
    let max_ms = MAX_TICK_MS.load(Ordering::Relaxed);

    // Cuántos ms extender el tick. Un plazo ya vencido (0) no debe dormir: el
    // scheduler reevaluará de inmediato. <2 ms no merece reprogramar (el tick
    // de 1 ms ya despierta a tiempo). `None` => extiende al máximo del HW.
    let extend = match next_wake_ms {
        Some(0) => return,
        Some(ms) if ms >= 2 => ms.min(max_ms),
        None => max_ms,
        _ => 0, // Some(1): tick normal de 1 ms basta.
    };

    if extend < 2 {
        cortex_m::asm::wfi();
        return;
    }

    // Programa el intervalo extendido bajo IRQs enmascaradas (la ISR no entra).
    let reload = (extend.saturating_mul(units)).saturating_sub(1) & 0x00FF_FFFF;
    cortex_m::interrupt::free(|_| {
        // SAFETY: SysTick es exclusivo de este módulo; sección crítica.
        let mut syst = unsafe { cortex_m::Peripherals::steal().SYST };
        TICK_MS.store(extend, Ordering::Relaxed);
        syst.set_reload(reload);
        syst.clear_current();
        let _ = syst.has_wrapped(); // limpia COUNTFLAG residual
    });

    // Duerme. Despierta por la ISR de SysTick (intervalo expirado) o por IRQ
    // externa. El `wfi` ocurre con IRQs habilitadas para que la ISR avance el
    // reloj normalmente si es ella quien despierta.
    cortex_m::asm::wfi();

    // Re-sincroniza el reloj contabilizando lo que falte. Si fue la ISR quien
    // despertó, ya puso TICK_MS=1 y restauró el reload: el swap ve 1 y no hace
    // nada. Si despertó una IRQ externa con el intervalo aún en vuelo, aquí
    // contabilizamos el tiempo parcial y restauramos el tick de 1 ms.
    cortex_m::interrupt::free(|_| {
        let pending = TICK_MS.swap(1, Ordering::Relaxed);
        if pending == 1 {
            return; // la ISR ya contabilizó el intervalo.
        }
        // SAFETY: sección crítica; SysTick exclusivo del módulo de tiempo.
        let mut syst = unsafe { cortex_m::Peripherals::steal().SYST };
        let elapsed_counts = if syst.has_wrapped() {
            // Envolvió mientras estábamos en la sección (IRQ pendiente que aún no
            // corrió): el intervalo completo transcurrió. Limpia el SysTick
            // pendiente para que la ISR enmascarada no vuelva a sumar al salir.
            // SAFETY: escritura del bit PENDSTCLR de ICSR.
            unsafe { core::ptr::write_volatile(SCB_ICSR, ICSR_PENDSTCLR) };
            reload.wrapping_add(1)
        } else {
            // Despertar parcial genuino: cuentas consumidas = reload - actual.
            reload.wrapping_sub(SYST::get_current())
        };
        let total = elapsed_counts.wrapping_add(REMAINDER.load(Ordering::Relaxed));
        let ms = total / units;
        REMAINDER.store(total % units, Ordering::Relaxed);
        MILLIS.fetch_add(ms, Ordering::Relaxed);
        // Restaura el tick de 1 ms.
        syst.set_reload(units.saturating_sub(1) & 0x00FF_FFFF);
        syst.clear_current();
    });
}
