//! Rugus crypto — chip-agnostic primitives with a pure-Rust software backend.
//!
//! # Backends
//!
//! - **Software** ([`software`]) — SHA-256 and a CSPRNG suitable for TLS handshakes.
//! - **Hardware** — STM32F7 CRYP/HASH/RNG are not wired yet; see module docs in
//!   `rugus-hal-stm32f7` ROADMAP. Callers should use [`SoftwareRng`] until then.
//!
//! # Invariants
//!
//! - No FFI; all algorithms are Rust crates (`sha2`, etc.).
//! - Secret material is not logged (`defmt` traces are absent here).

#![no_std]
#![warn(missing_docs)]

pub mod mac;
pub mod software;

pub use mac::{ct_eq, hmac_sha256};
pub use software::{Sha256, SoftwareRng};

/// SHA-256 digest (32 bytes).
pub type Digest256 = [u8; 32];
