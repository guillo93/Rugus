//! EXTI del botón de usuario B1 (PA0) — primer IRQ no-SysTick, gemelo del de F7.
//!
//! El botón B1 de la STM32F407G-DISC1 está en PA0 (activo alto, pull-down
//! externo): pulsarlo genera un flanco de subida. Lo enrutamos por EXTI0 → NVIC
//! → handler, que incrementa un contador de eventos atómico. Una tarea
//! privilegiada lo observa de forma cooperativa (`events()`), de modo que un IRQ
//! real de periférico llega a código de tarea sin tocar el camino del scheduler.
//!
//! Validación sin pulsar físicamente: [`Button::trigger_test`] escribe `SWIER`,
//! que pende el EXTI igual que un flanco real — análogo al loopback HDSEL del
//! USART. Así el camino NVIC→handler→contador se prueba por RTT en ambas placas.
//!
//! EXTI/SYSCFG son idénticos en F4/F7 (mismos offsets), por eso este módulo es
//! gemelo del de F7. Acceso MMIO directo, en la línea de [`crate::gpio`].

use crate::gpio::{Pin, PinConfig, Port, Pull};
use crate::pac::{interrupt, Interrupt};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;

/// `RCC->APB2ENR`: bit 14 habilita el reloj de SYSCFG (necesario para EXTICR).
const RCC_APB2ENR: u32 = 0x4002_3844;
const SYSCFGEN: u32 = 1 << 14;

/// Base de SYSCFG; EXTICR1 mapea EXTI0..3 a un puerto (4 bits c/u).
const SYSCFG_BASE: u32 = 0x4001_3800;
const SYSCFG_EXTICR1: u32 = 0x08;

/// Base del bloque EXTI (idéntica en F4/F7).
const EXTI_BASE: u32 = 0x4001_3C00;
const EXTI_IMR: u32 = 0x00;
const EXTI_RTSR: u32 = 0x08;
const EXTI_SWIER: u32 = 0x10;
const EXTI_PR: u32 = 0x14;

/// Línea EXTI del botón = PA0 → EXTI0.
const LINE: u32 = 0;
const LINE_BIT: u32 = 1 << LINE;

/// Eventos de botón entregados por el handler EXTI0. Productor único: la ISR;
/// consumidor: la tarea supervisora (lectura cooperativa).
static EVENTS: AtomicU32 = AtomicU32::new(0);

/// Botón de usuario B1 cableado a EXTI0 (flanco de subida).
pub struct Button {
    _pin: Pin,
}

impl Button {
    /// Configura PA0 como entrada, enruta EXTI0→PA, arma el flanco de subida y
    /// habilita el IRQ EXTI0 en el NVIC. Tras esto, pulsar el botón (o
    /// [`Self::trigger_test`]) incrementa el contador de [`events`].
    pub fn new() -> Self {
        // PA0 entrada (la placa tiene pull-down externo; sin pull interno).
        let pin = Pin::new(Port::A, LINE as u8, PinConfig::input(Pull::None));
        // SAFETY: registros MMIO de RCC/SYSCFG/EXTI; arranque single-thread.
        unsafe {
            let v = read_volatile(RCC_APB2ENR as *const u32);
            write_volatile(RCC_APB2ENR as *mut u32, v | SYSCFGEN);
            let _ = read_volatile(RCC_APB2ENR as *const u32);
            // EXTICR1 nibble 0 = 0b0000 → PA para EXTI0.
            let cr = read_volatile((SYSCFG_BASE + SYSCFG_EXTICR1) as *const u32);
            write_volatile((SYSCFG_BASE + SYSCFG_EXTICR1) as *mut u32, cr & !0xF);
            // Flanco de subida + desenmascarar la línea; limpia pendiente previa.
            exti_set(EXTI_PR, LINE_BIT);
            exti_or(EXTI_RTSR, LINE_BIT);
            exti_or(EXTI_IMR, LINE_BIT);
            NVIC::unmask(Interrupt::EXTI0);
        }
        Self { _pin: pin }
    }

    /// Pende el EXTI0 por software (`SWIER`) para autotest: dispara el handler
    /// igual que un flanco real, sin pulsar el botón.
    pub fn trigger_test(&self) {
        // SAFETY: SWIER es write-1-to-set sobre la línea; atómico por bit.
        unsafe { exti_or(EXTI_SWIER, LINE_BIT) }
    }
}

impl Default for Button {
    fn default() -> Self {
        Self::new()
    }
}

/// Número de eventos de botón entregados por la ISR desde el arranque.
pub fn events() -> u32 {
    EVENTS.load(Ordering::Relaxed)
}

/// Handler EXTI0: limpia la pendiente y contabiliza el evento. El efecto visible
/// (LED, log) lo decide la tarea que observa [`events`].
#[interrupt]
fn EXTI0() {
    // SAFETY: PR es write-1-to-clear; la ISR es el único escritor del contador.
    unsafe {
        exti_set(EXTI_PR, LINE_BIT);
    }
    EVENTS.fetch_add(1, Ordering::Relaxed);
}

/// Escribe `bits` tal cual (registros write-1: PR limpia, SWIER pende).
#[inline]
unsafe fn exti_set(off: u32, bits: u32) {
    unsafe { write_volatile((EXTI_BASE + off) as *mut u32, bits) }
}

/// RMW: añade `bits` a un registro de configuración (IMR/RTSR).
#[inline]
unsafe fn exti_or(off: u32, bits: u32) {
    unsafe {
        let v = read_volatile((EXTI_BASE + off) as *const u32);
        write_volatile((EXTI_BASE + off) as *mut u32, v | bits);
    }
}
