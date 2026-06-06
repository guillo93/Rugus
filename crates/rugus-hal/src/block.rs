//! Block device trait (F5.C) — almacenamiento direccionable por bloques.
//!
//! Contrato mínimo que un backend de almacenamiento (QSPI NOR flash, microSD…)
//! implementa para que capas superiores (un filesystem como `littlefs`, o un
//! log circular de faults) operen sin conocer el medio físico.
//!
//! Semántica estilo NOR flash, que es el mínimo común denominador:
//!
//! - **read**: lectura de bytes arbitrarios en cualquier dirección.
//! - **program**: escritura que solo puede pasar bits de `1`→`0`; el llamante
//!   debe haber borrado antes la zona. Granularidad de página
//!   ([`BlockDevice::PROG_SIZE`], típicamente 256 B en NOR).
//! - **erase**: pone una región a todos-`1` (`0xFF`). Granularidad de sector
//!   ([`BlockDevice::ERASE_SIZE`], típicamente 4 KiB en NOR).
//!
//! Las direcciones y longitudes se expresan en bytes desde el inicio del medio.

/// Dispositivo de almacenamiento direccionable por bloques.
///
/// Las constantes asociadas describen la geometría del medio. Un consumidor
/// (p. ej. un FS) debe respetar las granularidades de `program`/`erase`.
pub trait BlockDevice {
    /// Error específico del backend.
    type Error;

    /// Tamaño total del medio en bytes.
    fn capacity(&self) -> u64;

    /// Granularidad de programación (page program) en bytes.
    fn prog_size(&self) -> usize;

    /// Granularidad de borrado (sector erase) en bytes.
    fn erase_size(&self) -> usize;

    /// Lee `buf.len()` bytes desde `addr`.
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Programa `data` en `addr`. El llamante garantiza que la región estaba
    /// borrada (`0xFF`) y que `addr`/`data.len()` no cruzan un límite de página
    /// si el backend no lo soporta (ver doc del backend).
    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), Self::Error>;

    /// Borra el sector que contiene `addr` (pone `ERASE_SIZE` bytes a `0xFF`).
    fn erase_sector(&mut self, addr: u32) -> Result<(), Self::Error>;
}
