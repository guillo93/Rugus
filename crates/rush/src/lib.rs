//! `rush` — Rugus on-device shell (léxico v1, ANSI, parser, IDENTIFY).
//!
//! Capa de presentación del dispositivo. **No** accede a hardware: todos los
//! comandos mapean a `rugus_core::syscall::lite::user`. Incluye el protocolo
//! ligero `IDENTIFY`, que permite a un host (rugus-cli de escritorio) descubrir
//! un dispositivo Rugus por serie o BLE.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod ansi;
pub mod banner;
mod commands;
pub mod identify;
#[cfg(feature = "auth")]
pub mod session;

pub use ansi::Write;
#[cfg(feature = "auth")]
pub use commands::execute_authed;
pub use commands::{execute, parse, Command};
pub use identify::{write_signature, write_signature_ext, ENQ, PROTO_VERSION, SHELL_NAME};
#[cfg(feature = "auth")]
pub use session::{AuthHooks, Session};

/// Versión del léxico de la shell expuesta en `cosmos` y en `IDENTIFY`.
pub const CLI_VERSION: &str = "1.0.0";
