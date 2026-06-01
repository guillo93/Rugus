//! Timers de propósito general — base de tiempo µs (TIM2) y PWM (TIM3).
//!
//! Acceso MMIO directo en la línea de [`crate::gpio`]. La IP de timer GP es
//! idéntica en F4/F7 (mismos offsets), por eso este módulo es gemelo del de F4.
//! Ambos timers cuelgan de APB1; su reloj es `pclk1 * 2` cuando el prescaler de
//! APB1 ≠ 1 (es el caso en ambas placas), de ahí el parámetro `timer_clk`.
//!
//! - [`Timebase`] (TIM2, 32 bits): contador libre a 1 MHz → microsegundos
//!   monotónicos y `delay_us`, complemento de grano fino del SysTick (1 ms).
//! - [`PwmCheck`] (TIM3, 16 bits): genera PWM y se autovalida muestreando
//!   `CNT < CCR` sobre un periodo (la salida está alta en esa fracción), sin
//!   necesidad de osciloscopio ni de cablear un pin.

use core::ptr::{read_volatile, write_volatile};

const RCC_APB1ENR: u32 = 0x4002_3840;
const TIM2EN: u32 = 1 << 0;
const TIM3EN: u32 = 1 << 1;

const TIM2_BASE: u32 = 0x4000_0000;
const TIM3_BASE: u32 = 0x4000_0400;

// Offsets comunes de la IP de timer de propósito general.
const CR1: u32 = 0x00;
const EGR: u32 = 0x14;
const CCMR1: u32 = 0x18;
const CCER: u32 = 0x20;
const CNT: u32 = 0x24;
const PSC: u32 = 0x28;
const ARR: u32 = 0x2C;
const CCR1: u32 = 0x34;

const CR1_CEN: u32 = 1 << 0;
const EGR_UG: u32 = 1 << 0;
const CCER_CC1E: u32 = 1 << 0;
// CCMR1: OC1M = PWM modo 1 (0b110 en bits 6:4) + OC1PE (preload) en bit 3.
const CCMR1_PWM1: u32 = (0b110 << 4) | (1 << 3);

/// Base de tiempo de 1 MHz sobre TIM2 (32 bits): microsegundos monotónicos.
pub struct Timebase;

impl Timebase {
    /// Arranca TIM2 como contador libre a 1 MHz. `timer_clk` es el reloj del
    /// timer (`pclk1 * 2` en F4/F7 con prescaler APB1 ≠ 1).
    pub fn start(timer_clk: u32) -> Self {
        // SAFETY: registros MMIO de TIM2; arranque single-thread.
        unsafe {
            let v = read_volatile(RCC_APB1ENR as *const u32);
            write_volatile(RCC_APB1ENR as *mut u32, v | TIM2EN);
            let _ = read_volatile(RCC_APB1ENR as *const u32);
            write_reg(TIM2_BASE, PSC, timer_clk / 1_000_000 - 1);
            write_reg(TIM2_BASE, ARR, 0xFFFF_FFFF);
            write_reg(TIM2_BASE, EGR, EGR_UG); // recarga PSC/ARR
            write_reg(TIM2_BASE, CR1, CR1_CEN);
        }
        Self
    }

    /// Microsegundos transcurridos desde [`Self::start`] (envuelve a ~4295 s).
    #[inline]
    pub fn now_us(&self) -> u32 {
        // SAFETY: CNT es de solo lectura.
        unsafe { read_reg(TIM2_BASE, CNT) }
    }

    /// Espera bloqueante `us` microsegundos (aritmética envolvente).
    pub fn delay_us(&self, us: u32) {
        let start = self.now_us();
        while self.now_us().wrapping_sub(start) < us {}
    }
}

/// Generador PWM autovalidable sobre TIM3 CH1 (16 bits).
pub struct PwmCheck;

impl PwmCheck {
    /// Configura PWM modo 1 en TIM3_CH1: periodo `arr+1` ticks a 1 MHz, duty
    /// `ccr/(arr+1)`. La salida (interna; no se enruta a pin) está alta mientras
    /// `CNT < CCR`. `timer_clk` como en [`Timebase::start`].
    pub fn start(timer_clk: u32, arr: u16, ccr: u16) -> Self {
        // SAFETY: registros MMIO de TIM3; arranque single-thread.
        unsafe {
            let v = read_volatile(RCC_APB1ENR as *const u32);
            write_volatile(RCC_APB1ENR as *mut u32, v | TIM3EN);
            let _ = read_volatile(RCC_APB1ENR as *const u32);
            write_reg(TIM3_BASE, PSC, timer_clk / 1_000_000 - 1);
            write_reg(TIM3_BASE, ARR, arr as u32);
            write_reg(TIM3_BASE, CCR1, ccr as u32);
            write_reg(TIM3_BASE, CCMR1, CCMR1_PWM1);
            write_reg(TIM3_BASE, CCER, CCER_CC1E);
            write_reg(TIM3_BASE, EGR, EGR_UG);
            write_reg(TIM3_BASE, CR1, CR1_CEN);
        }
        Self
    }

    /// Estima el duty muestreando `CNT < CCR` sobre `samples` lecturas; devuelve
    /// la fracción de tiempo en alto en por mil (0..=1000). Valida la generación
    /// PWM (reloj/PSC/ARR/CCR/modo) sin osciloscopio.
    pub fn measure_duty_permille(&self, samples: u32) -> u32 {
        // SAFETY: CNT/CCR1/ARR de TIM3 son de solo lectura aquí.
        let (ccr, _arr) = unsafe { (read_reg(TIM3_BASE, CCR1), read_reg(TIM3_BASE, ARR)) };
        let mut high = 0u32;
        for _ in 0..samples {
            // SAFETY: lectura atómica de CNT.
            let cnt = unsafe { read_reg(TIM3_BASE, CNT) };
            if cnt < ccr {
                high += 1;
            }
        }
        high.saturating_mul(1000) / samples.max(1)
    }
}

#[inline]
unsafe fn read_reg(base: u32, off: u32) -> u32 {
    unsafe { read_volatile((base + off) as *const u32) }
}

#[inline]
unsafe fn write_reg(base: u32, off: u32, val: u32) {
    unsafe { write_volatile((base + off) as *mut u32, val) }
}
