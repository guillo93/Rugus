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
mod commands;
pub mod identify;

pub use ansi::Write;
pub use commands::{execute, parse, Command};
pub use identify::{write_signature, ENQ, PROTO_VERSION, SHELL_NAME};

/// Versión del léxico de la shell expuesta en `cosmos` y en `IDENTIFY`.
pub const CLI_VERSION: &str = "1.0.0";
