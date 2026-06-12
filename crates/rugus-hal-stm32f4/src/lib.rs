//! Rugus HAL implementation for STM32F4 family.
//!
//! G3 reference: STM32F407VGT6 on STM32F407G-DISC1. Drivers grow by milestone
//! (`docs/ROADMAP.md`):
//!
//! | Hito | Módulo añadido |
//! |------|----------------|
//! | G3   | `gpio`, `rcc` (blink) |
//! | G4+  | `eth`, `usb`, `cryp`, etc. as needed |
//!
//! Re-exporta el PAC `stm32f4` para uso interno. Los consumidores deben usar
//! los wrappers de este crate, no el PAC directamente.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub use stm32f4::stm32f407 as pac;

pub mod adc;
pub mod exti;
pub mod flash;
pub mod gpio;
pub mod iwdg;
pub mod rcc;
pub mod reset;
pub mod timer;
pub mod usart;
