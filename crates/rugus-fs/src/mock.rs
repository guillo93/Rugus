//! Backend RAM que emula la semántica de una NOR flash, para tests host.
//!
//! Reglas NOR reproducidas fielmente para cazar bugs reales del driver/FS:
//!
//! - **erase**: pone un sector entero a `0xFF`.
//! - **program**: solo puede cambiar bits `1`->`0` (AND con el dato). Programar
//!   sobre una zona ya escrita NO la "limpia"; refleja el comportamiento físico.
//! - **read**: lectura arbitraria.
//!
//! Permite además simular un **corte de energía** truncando la última operación
//! de programación (ver [`RamFlash::set_power_fail_after`]).

extern crate std;
use std::vec;
use std::vec::Vec;

use rugus_hal::BlockDevice;

/// Error del backend simulado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockError {
    /// Dirección/longitud fuera del medio.
    OutOfRange,
    /// Corte de energía simulado durante un `program`.
    PowerFail,
}

/// Flash NOR simulada en RAM.
pub struct RamFlash {
    data: Vec<u8>,
    erase_size: usize,
    prog_size: usize,
    /// Si es `Some(n)`, el próximo `program` solo escribe `n` bytes y luego
    /// devuelve [`MockError::PowerFail`] (simula corte de energía a mitad).
    power_fail_after: Option<usize>,
}

impl RamFlash {
    /// Crea una flash de `sectors` sectores de `erase_size` bytes; página
    /// `prog_size`. Arranca toda borrada (`0xFF`), como una NOR virgen.
    pub fn new(sectors: usize, erase_size: usize, prog_size: usize) -> Self {
        Self {
            data: vec![0xFF; sectors * erase_size],
            erase_size,
            prog_size,
            power_fail_after: None,
        }
    }

    /// Programa el próximo `program` para que escriba como mucho `bytes` y
    /// después falle, emulando una pérdida de energía a mitad de la escritura.
    pub fn set_power_fail_after(&mut self, bytes: usize) {
        self.power_fail_after = Some(bytes);
    }

    /// Acceso de solo lectura al medio (para aserciones de test).
    pub fn raw(&self) -> &[u8] {
        &self.data
    }
}

impl BlockDevice for RamFlash {
    type Error = MockError;

    fn capacity(&self) -> u64 {
        self.data.len() as u64
    }

    fn prog_size(&self) -> usize {
        self.prog_size
    }

    fn erase_size(&self) -> usize {
        self.erase_size
    }

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MockError> {
        let start = addr as usize;
        let end = start.checked_add(buf.len()).ok_or(MockError::OutOfRange)?;
        if end > self.data.len() {
            return Err(MockError::OutOfRange);
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), MockError> {
        let start = addr as usize;
        let end = start.checked_add(data.len()).ok_or(MockError::OutOfRange)?;
        if end > self.data.len() {
            return Err(MockError::OutOfRange);
        }
        let limit = self.power_fail_after.take().unwrap_or(data.len());
        let written = limit.min(data.len());
        for (i, &b) in data.iter().take(written).enumerate() {
            // Semántica NOR: solo se pueden bajar bits (AND).
            self.data[start + i] &= b;
        }
        if written < data.len() {
            return Err(MockError::PowerFail);
        }
        Ok(())
    }

    fn erase_sector(&mut self, addr: u32) -> Result<(), MockError> {
        let sector = (addr as usize) / self.erase_size;
        let base = sector * self.erase_size;
        if base >= self.data.len() {
            return Err(MockError::OutOfRange);
        }
        for b in &mut self.data[base..base + self.erase_size] {
            *b = 0xFF;
        }
        Ok(())
    }
}
