//! Rugus HAL implementation for STM32F1 family.
//!
//! Rugus lite reference: STM32F103C8T6 on the Blue Pill. Drivers grow by
//! milestone (`docs/ROADMAP.md`):
//!
//! | Hito | Módulo añadido |
//! |------|----------------|
//! | F103 kickoff | `gpio`, `rcc` (blink) |
//!
//! Re-exporta el PAC `stm32f1` para uso interno. Los consumidores deben usar
//! los wrappers de este crate, no el PAC directamente.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub use stm32f1::stm32f103 as pac;

pub mod gpio;
pub mod gpio_raw;
pub mod hm20;
pub mod i2c;
pub mod rcc;
pub mod spi_sd;
pub mod uart;
pub mod uart2;
pub mod wdt;
