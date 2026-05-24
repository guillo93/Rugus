//! Syscall ABI v0.1. Ver `docs/SYSCALL_ABI.md` para el spec completo.

/// Versión actual del ABI expuesta a userspace.
///
/// Apps pueden leerla y abortar limpiamente si requieren una revisión más
/// alta. Bumps mayores indican ruptura; bumps menores son aditivos.
pub const ABI_VERSION: u16 = 0x0001; // 0.1

/// Identificadores de syscall. Los valores numéricos son parte del ABI
/// estable post-G2 — no renumerar tras 1.0.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Id {
    YieldNow      = 0x00,
    SleepMs       = 0x01,
    TaskId        = 0x02,
    Log           = 0x03,
    IpcSend       = 0x10,
    IpcRecv       = 0x11,
    NetSocket     = 0x30,
    NetConnect    = 0x31,
    NetSend       = 0x32,
    NetRecv       = 0x33,
    CryptoSign    = 0x40,
    RngFill       = 0x41,
    PanicApp      = 0xFE,
    Extended      = 0xFF,
}
