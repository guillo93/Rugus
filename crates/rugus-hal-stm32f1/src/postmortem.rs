//! Post-mortem persistente sobre los *backup registers* del F103.
//!
//! Los registros BKP_DRx viven en el dominio de respaldo: sobreviven a un reset
//! del sistema (incluido el del IWDG) mientras VDD/VBAT se mantenga. Eso permite
//! un diagnóstico que cruza el reinicio: si una tarea faulta y el watchdog acaba
//! reseteando, al volver a arrancar podemos decir *por qué* se reinició y qué
//! tarea cayó, sin RTT ni depurador conectado.
//!
//! - [`read_reset_cause`] lee y limpia los flags de RCC_CSR (causa del último
//!   reset a nivel de hardware).
//! - [`save_fault`] graba la causa de un fault (kind + task id) en BKP_DR1/DR2;
//!   pensado para llamarse desde el fault hook, antes de matar la tarea.
//! - [`take_fault`] lee y limpia ese registro al arrancar.
//!
//! Acceso al dominio de respaldo: requiere habilitar los relojes de PWR y BKP y
//! levantar `PWR_CR.DBP` para desproteger la escritura. Estas funciones usan los
//! punteros del PAC directamente (en el fault hook no hay `Peripherals`), de ahí
//! el `unsafe` interno acotado.

use crate::pac;

/// Marca en el byte alto de BKP_DR1 que el registro contiene un fault válido.
const FAULT_MAGIC: u16 = 0xF500;
const FAULT_MASK: u16 = 0xFF00;

/// Causa del último reset, según RCC_CSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetCause {
    /// Encendido / brown-out (POR/PDR).
    PowerOn,
    /// Pin NRST externo.
    Pin,
    /// Reset por software (SYSRESETREQ).
    Software,
    /// Independent watchdog (IWDG) — típicamente un cuelgue contenido.
    IndependentWatchdog,
    /// Window watchdog (WWDG).
    WindowWatchdog,
    /// Salida de modo bajo consumo.
    LowPower,
    /// Sin flags reconocidos.
    Unknown,
}

impl ResetCause {
    /// Nombre corto y estable para logs/telemetría.
    pub const fn name(self) -> &'static str {
        match self {
            Self::PowerOn => "power-on",
            Self::Pin => "pin-reset",
            Self::Software => "software",
            Self::IndependentWatchdog => "watchdog",
            Self::WindowWatchdog => "wwdg",
            Self::LowPower => "low-power",
            Self::Unknown => "unknown",
        }
    }
}

/// Registro de fault recuperado del dominio de respaldo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultRecord {
    /// Código de [`rugus_core::fault::FaultKind`] (1=MemManage…4=HardFault).
    pub kind: u8,
    /// Id de la tarea que faultó.
    pub task: u8,
}

/// Lee y limpia los flags de causa de reset de RCC_CSR.
///
/// Devuelve la causa de mayor prioridad presente. Tras leer, escribe `RMVF`
/// para que el próximo arranque no herede flags viejos.
pub fn read_reset_cause(rcc: &pac::RCC) -> ResetCause {
    let csr = rcc.csr.read();
    let cause = if csr.lpwrrstf().bit() {
        ResetCause::LowPower
    } else if csr.wwdgrstf().bit() {
        ResetCause::WindowWatchdog
    } else if csr.iwdgrstf().bit() {
        ResetCause::IndependentWatchdog
    } else if csr.sftrstf().bit() {
        ResetCause::Software
    } else if csr.porrstf().bit() {
        ResetCause::PowerOn
    } else if csr.pinrstf().bit() {
        ResetCause::Pin
    } else {
        ResetCause::Unknown
    };
    rcc.csr.modify(|_, w| w.rmvf().set_bit());
    cause
}

/// Habilita el acceso de escritura al dominio de respaldo (relojes PWR+BKP y
/// `PWR_CR.DBP`). Idempotente.
///
/// # Safety
///
/// Usa punteros del PAC; solo debe llamarse en contexto single-thread (boot o
/// fault hook), no concurrentemente con otro acceso a RCC/PWR.
unsafe fn enable_backup_access() {
    let rcc = unsafe { &*pac::RCC::ptr() };
    let pwr = unsafe { &*pac::PWR::ptr() };
    rcc.apb1enr
        .modify(|_, w| w.pwren().set_bit().bkpen().set_bit());
    let _ = rcc.apb1enr.read().bits();
    pwr.cr.modify(|_, w| w.dbp().set_bit());
}

/// Graba un fault (kind + task) en BKP_DR1/DR2. Llamar desde el fault hook
/// antes de matar la tarea, para que sobreviva al posible reset del watchdog.
///
/// # Safety
///
/// Contexto de fault (handler mode), single-thread; escribe el dominio de
/// respaldo vía punteros del PAC.
pub unsafe fn save_fault(kind: u8, task: u8) {
    unsafe {
        enable_backup_access();
        let bkp = &*pac::BKP::ptr();
        bkp.dr[0].write(|w| w.d().bits(FAULT_MAGIC | u16::from(kind)));
        bkp.dr[1].write(|w| w.d().bits(u16::from(task)));
    }
}

/// Lee y limpia el registro de fault del dominio de respaldo. Devuelve `Some`
/// solo si el byte de magia está presente (hubo un fault grabado).
pub fn take_fault(rcc: &pac::RCC, pwr: &pac::PWR, bkp: &pac::BKP) -> Option<FaultRecord> {
    rcc.apb1enr
        .modify(|_, w| w.pwren().set_bit().bkpen().set_bit());
    let _ = rcc.apb1enr.read().bits();
    pwr.cr.modify(|_, w| w.dbp().set_bit());
    let dr1 = bkp.dr[0].read().d().bits();
    if dr1 & FAULT_MASK != FAULT_MAGIC {
        return None;
    }
    let kind = (dr1 & 0x00FF) as u8;
    let task = (bkp.dr[1].read().d().bits() & 0x00FF) as u8;
    bkp.dr[0].write(|w| w.d().bits(0));
    bkp.dr[1].write(|w| w.d().bits(0));
    Some(FaultRecord { kind, task })
}
