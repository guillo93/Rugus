//! Rugus HAL traits — contrato público que cada `rugus-hal-<chip>`
//! implementa.
//!
//! Este crate es **solo definiciones**: cero `unsafe`, cero dependencias
//! pesadas. Un proyecto puede consumir `rugus-hal` sin arrastrar
//! `rugus-core` (útil para drivers third-party que quieran ser compatibles
//! con el ecosistema sin atarse al kernel).
//!
//! Ver `docs/HAL_TRAITS.md` para el contrato completo y semantic
//! versioning.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod crypto;
pub mod digital;
pub mod eth;
pub mod serial;

pub use crypto::CryptoRng;

pub use digital::GpioPin;
pub use eth::EthMac;
pub use serial::SerialPort;
