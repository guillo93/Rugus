//! Rugus HAL implementation para STM32F7 family.
//!
//! Estado G0: GPIO suficiente para parpadear LEDs. Drivers que crecerán
//! por fase (`docs/ROADMAP.md`):
//!
//! | Hito | Módulo añadido |
//! |------|----------------|
//! | G0   | `gpio` (mínimo) |
//! | G1   | `rcc` (clocks), `cache`, `fmc` (SDRAM), `flash`, `systick` |
//! | G2   | `nvic`, hooks para MPU del arch backend |
//! | G4   | `ltdc`, `dma2d`, `i2c`, `eth`, `cryp`, `hash`, `rng` |
//! | G4+  | `jpeg`, `usb_hs` |
//!
//! Re-exporta el PAC `stm32f7` para uso interno. Los consumidores deben
//! consumir los wrappers de este crate, no el PAC directamente.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub use stm32f7::stm32f7x9 as pac;

pub mod adc;
pub mod cache;
pub mod eth;
pub mod exti;
pub mod fmc;
pub mod gpio;
pub mod iwdg;
pub mod qspi;
pub mod rcc;
pub mod reset;
pub mod timer;
pub mod usart;
