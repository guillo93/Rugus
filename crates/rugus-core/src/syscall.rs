//! Syscall ABI v0.1 — dispatch y trampolines userland. Ver `docs/SYSCALL_ABI.md`.

use crate::sched::TaskId;
use crate::Errno;

/// Versión actual del ABI expuesta a userspace.
pub const ABI_VERSION: u16 = 0x0001;

/// Identificadores de syscall. Los valores numéricos son parte del ABI
/// estable post-G2 — no renumerar tras 1.0.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Id {
    YieldNow = 0x00,
    SleepMs = 0x01,
    TaskId = 0x02,
    Log = 0x03,
    IpcSend = 0x10,
    IpcRecv = 0x11,
    NetSocket = 0x30,
    NetConnect = 0x31,
    NetSend = 0x32,
    NetRecv = 0x33,
    CryptoSign = 0x40,
    RngFill = 0x41,
    PanicApp = 0xFE,
    Extended = 0xFF,
}

impl Id {
    /// Decodifica raw imm8 del SVC.
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0x00 => Some(Self::YieldNow),
            0x01 => Some(Self::SleepMs),
            0x02 => Some(Self::TaskId),
            0x03 => Some(Self::Log),
            0x10 => Some(Self::IpcSend),
            0x11 => Some(Self::IpcRecv),
            0x30 => Some(Self::NetSocket),
            0x31 => Some(Self::NetConnect),
            0x32 => Some(Self::NetSend),
            0x33 => Some(Self::NetRecv),
            0x40 => Some(Self::CryptoSign),
            0x41 => Some(Self::RngFill),
            0xFE => Some(Self::PanicApp),
            0xFF => Some(Self::Extended),
            _ => None,
        }
    }
}

/// Hooks registrados por el kernel / ejemplo antes de arrancar tareas.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Cede CPU (cooperativo).
    pub yield_now: fn(),
    /// ID de la tarea en ejecución.
    pub current_task_id: fn() -> TaskId,
    /// Dominio lógico de la tarea en ejecución.
    pub current_domain: fn() -> crate::Domain,
}

static mut HOOKS: Option<Hooks> = None;

/// Registra callbacks del scheduler. Llamar una vez desde `main`.
///
/// # Safety
///
/// Solo desde main, antes de `sched.start()`.
pub unsafe fn register(hooks: Hooks) {
    unsafe {
        HOOKS = Some(hooks);
    }
}

/// ID de tarea actual (0 si no hay hooks).
pub fn current_task_id() -> TaskId {
    // SAFETY: lectura de static; hooks inmutables tras init.
    unsafe { HOOKS.map(|h| (h.current_task_id)()).unwrap_or(TaskId(0)) }
}

/// Dominio de la tarea actual.
pub fn current_domain() -> crate::Domain {
    // SAFETY: lectura de static; hooks inmutables tras init.
    unsafe {
        HOOKS
            .map(|h| (h.current_domain)())
            .unwrap_or(crate::Domain::Kernel)
    }
}

/// Dispatch central invocado desde el SVC handler (arch backend).
pub fn dispatch(id: Id, args: [u32; 4]) -> i32 {
    match id {
        Id::YieldNow => {
            // SAFETY: hook registrado antes de userland.
            unsafe {
                if let Some(h) = HOOKS {
                    (h.yield_now)();
                }
            }
            0
        }
        Id::TaskId => {
            let id = current_task_id();
            id.0 as i32
        }
        Id::SleepMs
        | Id::Log
        | Id::IpcSend
        | Id::IpcRecv
        | Id::NetSocket
        | Id::NetConnect
        | Id::NetSend
        | Id::NetRecv
        | Id::CryptoSign
        | Id::RngFill
        | Id::Extended => {
            let _ = args;
            Errno::Einval as i32
        }
        Id::PanicApp => {
            let _ = args;
            Errno::Einval as i32
        }
    }
}

/// Trampolines userland — ejecutan `SVC #imm8` (ARMv7-M ABI).
pub mod user {
    use super::Id;

    /// Cede el CPU al scheduler (`Id::YieldNow`).
    #[inline(always)]
    pub fn yield_now() -> i32 {
        svc_imm(Id::YieldNow as u8)
    }

    /// Retorna el ID de la tarea actual (`Id::TaskId`).
    #[inline(always)]
    pub fn task_id() -> i32 {
        svc_imm(Id::TaskId as u8)
    }

    #[inline(always)]
    fn svc_imm(imm: u8) -> i32 {
        match imm {
            0x00 => svc0_00(),
            0x02 => svc0_02(),
            _ => crate::Errno::Einval as i32,
        }
    }

    #[inline(always)]
    fn svc0_00() -> i32 {
        let ret: i32;
        unsafe {
            core::arch::asm!(
                "svc 0",
                lateout("r0") ret,
                options(nomem, nostack)
            );
        }
        ret
    }

    #[inline(always)]
    fn svc0_02() -> i32 {
        let ret: i32;
        unsafe {
            core::arch::asm!(
                "svc 2",
                lateout("r0") ret,
                options(nomem, nostack)
            );
        }
        ret
    }
}
