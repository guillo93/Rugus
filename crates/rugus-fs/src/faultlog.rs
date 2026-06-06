//! Log circular de faults persistente sobre [`Rufs`] (F5.C.3).
//!
//! Complementa la telemetría volátil en `.uninit` (que se pierde al cortar la
//! energía) guardando un anillo de los últimos `CAP` eventos de fault en la NOR
//! flash. Cada evento ([`FaultRecord`]) son 12 bytes: un `seq` monotónico
//! global, un `kind` (código de causa) y un `arg` (dato contextual: PC, dirección
//! fallida, id de tarea…). El log se mantiene como un conjunto de claves
//! `flt_NN` (una por ranura del anillo) más una clave de cabecera `flt_head` que
//! persiste el contador `seq`; al grabar, la ranura elegida es `seq % CAP`, de
//! modo que las entradas más viejas se sobrescriben en orden (FIFO acotado).
//!
//! Power-loss-safe por construcción: hereda la semántica append+CRC de [`Rufs`]
//! (una grabación truncada se descarta al remontar; las anteriores sobreviven).

use crate::{Error, Rufs};
use rugus_hal::BlockDevice;

/// Tamaño serializado de un registro de fault en la FS (LE): `seq`+`kind`+`arg`.
pub const RECORD_LEN: usize = 12;

/// Prefijo de las claves de ranura (`flt_` + índice decimal de 2 dígitos).
const SLOT_PREFIX: &[u8] = b"flt_";
/// Clave de cabecera: persiste el contador `seq` (siguiente número a asignar).
const HEAD_KEY: &[u8] = b"flt_head";

/// Un evento de fault persistido.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FaultRecord {
    /// Secuencia monotónica global (orden de ocurrencia; nunca se reinicia).
    pub seq: u32,
    /// Código de causa (definido por el llamante: HardFault, MemManage, panic…).
    pub kind: u32,
    /// Dato contextual asociado (PC, MMFAR/BFAR, id de tarea, código de panic…).
    pub arg: u32,
}

impl FaultRecord {
    fn to_bytes(self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        b[0..4].copy_from_slice(&self.seq.to_le_bytes());
        b[4..8].copy_from_slice(&self.kind.to_le_bytes());
        b[8..12].copy_from_slice(&self.arg.to_le_bytes());
        b
    }

    fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < RECORD_LEN {
            return None;
        }
        Some(Self {
            seq: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            kind: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            arg: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        })
    }
}

/// Log circular de faults sobre una [`Rufs`]. `CAP` = número de ranuras del
/// anillo (entradas retenidas). Debe ser `1..=99` (la clave usa 2 dígitos).
pub struct FaultLog<const CAP: usize> {
    /// Próximo número de secuencia a asignar (= total de faults registrados).
    seq: u32,
}

impl<const CAP: usize> FaultLog<CAP> {
    /// Abre el log sobre `fs`, recuperando el contador `seq` persistido (0 si es
    /// la primera vez). No escribe nada.
    pub fn open<D: BlockDevice, const N: usize>(
        fs: &mut Rufs<D, N>,
    ) -> Result<Self, Error<D::Error>> {
        const { assert!(CAP >= 1 && CAP <= 99, "FaultLog CAP debe estar en 1..=99") };
        let mut buf = [0u8; 4];
        let seq = match fs.get(HEAD_KEY, &mut buf) {
            Ok(4) => u32::from_le_bytes(buf),
            Ok(_) => 0,
            Err(Error::NotFound) => 0,
            Err(e) => return Err(e),
        };
        Ok(Self { seq })
    }

    /// Registra un fault (`kind`, `arg`) en la siguiente ranura del anillo y
    /// persiste el contador. Devuelve el `seq` asignado a este evento.
    pub fn record<D: BlockDevice, const N: usize>(
        &mut self,
        fs: &mut Rufs<D, N>,
        kind: u32,
        arg: u32,
    ) -> Result<u32, Error<D::Error>> {
        let seq = self.seq;
        let rec = FaultRecord { seq, kind, arg };
        let slot = (seq as usize) % CAP;
        let key = slot_key(slot);
        fs.set(&key, &rec.to_bytes())?;
        self.seq = seq.wrapping_add(1);
        fs.set(HEAD_KEY, &self.seq.to_le_bytes())?;
        Ok(seq)
    }

    /// Total de faults registrados desde el primer arranque (contador `seq`).
    pub fn total(&self) -> u32 {
        self.seq
    }

    /// Número de registros vivos en el anillo (`min(total, CAP)`).
    pub fn stored(&self) -> usize {
        (self.seq as usize).min(CAP)
    }

    /// Invoca `f(record)` por cada entrada presente en el anillo, de la más
    /// antigua a la más reciente.
    pub fn for_each<D: BlockDevice, const N: usize, F: FnMut(FaultRecord)>(
        &self,
        fs: &mut Rufs<D, N>,
        mut f: F,
    ) -> Result<(), Error<D::Error>> {
        let stored = self.stored();
        // La ranura más antigua es la que se sobrescribirá a continuación cuando
        // el anillo está lleno; si aún no se ha dado la vuelta, empieza en 0.
        let start = (self.seq as usize).saturating_sub(CAP);
        for i in 0..stored {
            let slot = (start + i) % CAP;
            let key = slot_key(slot);
            let mut buf = [0u8; RECORD_LEN];
            match fs.get(&key, &mut buf) {
                Ok(RECORD_LEN) => {
                    if let Some(rec) = FaultRecord::from_bytes(&buf) {
                        f(rec);
                    }
                }
                Ok(_) | Err(Error::NotFound) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// Construye la clave de ranura `flt_NN` (índice decimal de 2 dígitos).
fn slot_key(slot: usize) -> heapless::Vec<u8, 8> {
    let mut key = heapless::Vec::new();
    let _ = key.extend_from_slice(SLOT_PREFIX);
    let _ = key.push(b'0' + (slot / 10) as u8);
    let _ = key.push(b'0' + (slot % 10) as u8);
    key
}
