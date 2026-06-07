//! `rugus-proto` — núcleo del protocolo Rugus para el **host**.
//!
//! Comparte el contrato de la shell on-device `rush` (ver crate `rush` en el
//! firmware) pero del lado del PC: parseo del handshake `IDENTIFY`, ensamblado
//! de líneas/frames desde un transporte (serie/BLE), modelo de comandos y
//! modelo de render (estilos ANSI → spans).
//!
//! Es `std` y agnóstico del transporte: no conoce `serialport` ni `btleplug`.
//! El binario `rugus-cli` aporta los transportes y la TUI.

pub mod auth;
pub mod command;
pub mod frame;
pub mod identify;
pub mod render;

pub use auth::{compute_proof, compute_proof_hex, decode_hex, encode_hex, PROOF_LEN};
pub use command::Command;
pub use frame::LineAssembler;
pub use identify::{parse_signature, Signature, SignatureError, ENQ, IDENTIFY_REQUEST};
pub use render::{Span, Style, StyledLine};
