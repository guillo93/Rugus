//! Rugus kernel core (arch-agnostic).
//!
//! Define el trait [`Arch`] que cada crate `rugus-arch-<isa>` implementa,
//! y los tipos públicos del syscall ABI (`syscall::Id`, [`Errno`]).
//!
//! Esta capa **no** depende de ningún PAC ni de `cortex-m`. Toda
//! funcionalidad específica de CPU pasa por [`Arch`].

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod arch;
pub mod heap;
pub mod sched;
pub mod syscall;

pub use arch::Arch;

/// Errores visibles al espacio de usuario vía syscall. Mirrors negative
/// `i32` values en `docs/SYSCALL_ABI.md`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    /// Invalid argument.
    Einval = -1,
    /// Resource busy.
    Ebusy = -2,
    /// Operation timed out.
    Etimedout = -3,
    /// Host unreachable.
    Ehostunreach = -4,
    /// Permission denied.
    Edenied = -5,
    /// Overflow.
    Eoverflow = -6,
    /// Out of memory.
    Enomem = -7,
    /// Bad user pointer (rejected by MPU/MMU).
    Efault = -8,
}
