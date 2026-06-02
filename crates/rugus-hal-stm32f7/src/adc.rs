//! ADC1 — lectura de la referencia interna VREFINT (canal 17), gemelo del de F4.
//!
//! Acceso MMIO directo en la línea de [`crate::gpio`]. La IP del ADC es idéntica
//! en F4/F7 (mismos offsets), por eso este módulo es gemelo del de F4.
//!
//! VREFINT (~1.21 V) es una referencia interna del chip cableada a `ADC1_IN17`:
//! convertirla valida la cadena completa del ADC (reloj, encendido, secuencia,
//! muestreo, EOC) sin cablear ningún pin ni fuente externa — la misma filosofía
//! que el loopback HDSEL del USART o el SWIER del EXTI.

use core::ptr::{read_volatile, write_volatile};

/// `RCC->APB2ENR`: bit 8 habilita el reloj de ADC1.
const RCC_APB2ENR: u32 = 0x4002_3844;
const ADC1EN: u32 = 1 << 8;

const ADC1_BASE: u32 = 0x4001_2000;
/// Registro común de los ADC (TSVREFE habilita VREFINT y el sensor de temp).
const ADC_CCR: u32 = 0x4001_2304;
const CCR_TSVREFE: u32 = 1 << 23;

// Offsets de ADC1.
const SR: u32 = 0x00;
const CR2: u32 = 0x08;
const SMPR1: u32 = 0x0C;
const SQR1: u32 = 0x2C;
const SQR3: u32 = 0x34;
const DR: u32 = 0x4C;

const SR_EOC: u32 = 1 << 1;
const CR2_ADON: u32 = 1 << 0;
const CR2_SWSTART: u32 = 1 << 30;

/// Canal interno de VREFINT.
const CH_VREFINT: u32 = 17;

/// ADC1 configurado para conversión única de VREFINT.
pub struct Adc;

impl Adc {
    /// Enciende ADC1, habilita VREFINT y programa una secuencia de 1 conversión
    /// del canal 17 con tiempo de muestreo largo (VREFINT tiene impedancia alta).
    pub fn new() -> Self {
        // SAFETY: registros MMIO de RCC/ADC; arranque single-thread.
        unsafe {
            let v = read_volatile(RCC_APB2ENR as *const u32);
            write_volatile(RCC_APB2ENR as *mut u32, v | ADC1EN);
            let _ = read_volatile(RCC_APB2ENR as *const u32);

            write_volatile(
                ADC_CCR as *mut u32,
                read_volatile(ADC_CCR as *const u32) | CCR_TSVREFE,
            );
            // Tiempo de muestreo máx (0b111 = 480 ciclos) para el canal 17 (SMP17
            // en SMPR1 bits 23:21). VREFINT necesita muestreo largo.
            write_reg(SMPR1, 0b111 << 21);
            write_reg(SQR1, 0); // L=0 → 1 conversión en la secuencia regular.
            write_reg(SQR3, CH_VREFINT); // 1.ª conversión = canal 17.
            write_reg(CR2, CR2_ADON); // enciende el ADC.
        }
        // tSTAB tras ADON: espera breve a que el ADC se estabilice.
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        Self
    }

    /// Lanza una conversión de VREFINT y devuelve el valor crudo de 12 bits.
    pub fn read_vrefint_raw(&self) -> u16 {
        // SAFETY: registros MMIO de ADC1; secuencia de conversión única.
        unsafe {
            write_reg(SR, 0); // limpia EOC previo.
            write_reg(CR2, CR2_ADON | CR2_SWSTART);
            while read_reg(SR) & SR_EOC == 0 {}
            (read_reg(DR) & 0xFFF) as u16
        }
    }
}

impl Default for Adc {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
unsafe fn read_reg(off: u32) -> u32 {
    unsafe { read_volatile((ADC1_BASE + off) as *const u32) }
}

#[inline]
unsafe fn write_reg(off: u32, val: u32) {
    unsafe { write_volatile((ADC1_BASE + off) as *mut u32, val) }
}
