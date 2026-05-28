//! Rugus CLI — capa de presentación (léxico v1, ANSI, parser).
//!
//! **No** accede a hardware. Todos los comandos mapean a `rugus_core::syscall::lite::user`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod ansi;
mod commands;

pub use ansi::Write;
pub use commands::{execute, parse, Command};

/// Versión del léxico CLI expuesta en `cosmos`.
pub const CLI_VERSION: &str = "1.0.0";
