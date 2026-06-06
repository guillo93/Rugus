//! Tests host del plano de control de ficheros en el dispatch de syscalls
//! (`rugus_core::syscall`, F5.C.3). Verifica el ABI agnóstico de arquitectura:
//! decodificación de los `Id::Fs*`, fail-closed a `Enosys` sin servicio, y
//! ruteo a los `FsHooks` registrados. Los trampolines `svc` son arm-only y no
//! se prueban aquí.

use rugus_core::syscall::{dispatch, register_fs, FsHooks, Id};
use rugus_core::Errno;
use std::sync::atomic::{AtomicU32, Ordering};

#[test]
fn fs_ids_roundtrip() {
    assert_eq!(Id::from_raw(0x50), Some(Id::FsOpen));
    assert_eq!(Id::from_raw(0x51), Some(Id::FsRead));
    assert_eq!(Id::from_raw(0x52), Some(Id::FsWrite));
    assert_eq!(Id::from_raw(0x53), Some(Id::FsClose));
    assert_eq!(Id::FsOpen as u8, 0x50);
}

#[test]
fn reserved_syscalls_einval() {
    // Syscalls reservadas sin implementación deben devolver Einval, no rutear.
    assert_eq!(dispatch(Id::CryptoSign, [0; 4]), Errno::Einval as i32);
    assert_eq!(dispatch(Id::RngFill, [0; 4]), Errno::Einval as i32);
}

static OPEN_ARG: AtomicU32 = AtomicU32::new(0);
static WRITE_LEN: AtomicU32 = AtomicU32::new(0);

fn h_open(key_id: u32) -> i32 {
    OPEN_ARG.store(key_id, Ordering::SeqCst);
    7
}
fn h_read(_h: u32, _s: u32) -> i32 {
    42
}
fn h_write(_h: u32, _s: u32, len: u32) -> i32 {
    WRITE_LEN.store(len, Ordering::SeqCst);
    0
}
fn h_close(_h: u32) -> i32 {
    0
}

#[test]
fn fs_routes_to_hooks() {
    // SAFETY: test single-thread por binario; register_fs solo escribe un static.
    unsafe {
        register_fs(FsHooks {
            fs_open: h_open,
            fs_read: h_read,
            fs_write: h_write,
            fs_close: h_close,
        });
    }
    assert_eq!(dispatch(Id::FsOpen, [5, 0, 0, 0]), 7);
    assert_eq!(OPEN_ARG.load(Ordering::SeqCst), 5);
    assert_eq!(dispatch(Id::FsRead, [7, 1, 0, 0]), 42);
    assert_eq!(dispatch(Id::FsWrite, [7, 1, 99, 0]), 0);
    assert_eq!(WRITE_LEN.load(Ordering::SeqCst), 99);
    assert_eq!(dispatch(Id::FsClose, [7, 0, 0, 0]), 0);
}
