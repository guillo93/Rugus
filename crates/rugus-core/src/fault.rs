//! Reportes de fault desde el backend arch hacia el scheduler.

use crate::domain::Domain;
use crate::sched::TaskId;

/// Tipo de excepción Cortex-M reportada al kernel.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// HardFault.
    HardFault = 0,
    /// MemManage (MPU / acceso prohibido).
    MemManage = 1,
    /// BusFault.
    BusFault = 2,
    /// UsageFault.
    UsageFault = 3,
}

impl FaultKind {
    /// Etiqueta para logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::HardFault => "HardFault",
            Self::MemManage => "MemManage",
            Self::BusFault => "BusFault",
            Self::UsageFault => "UsageFault",
        }
    }
}

/// Datos mínimos que el kernel registra cuando una tarea faulta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultReport {
    /// Excepción concreta.
    pub kind: FaultKind,
    /// Program counter al faultear.
    pub pc: u32,
    /// Dominio lógico de la tarea faultante.
    pub domain: Domain,
    /// Tarea identificada por el scheduler.
    pub task_id: TaskId,
    /// Dirección que provocó el fault (MMFAR/BFAR), si el HW la marcó válida.
    /// `None` para UsageFault/HardFault sin dirección asociada o cuando el bit
    /// de validez (MMARVALID/BFARVALID) estaba en 0.
    pub addr: Option<u32>,
}
