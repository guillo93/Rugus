//! SVC handler — dispatch syscalls ABI v0.1 desde userland.

use core::arch::global_asm;
use core::ptr;
use rugus_core::syscall;
use rugus_core::Errno;

/// Decodifica el inmediato SVC (#imm8) apuntado por `pc` (Thumb).
#[inline]
pub fn svc_immediate_at_pc(pc: u32) -> u8 {
    let instr_addr = (pc & !1) as *const u16;
    // SAFETY: PC del frame de excepción apunta a la instrucción SVC.
    let half = unsafe { ptr::read_volatile(instr_addr) };
    (half & 0xFF) as u8
}

/// Entrada Rust invocada desde el vector SVC (ASM).
///
/// `psp` apunta al frame auto-guardado en el stack de la tarea.
#[no_mangle]
pub extern "C" fn rugus_svc_handler(psp: u32) -> u32 {
    let pc = crate::fault::stacked_pc(psp);
    let id_raw = svc_immediate_at_pc(pc.wrapping_sub(2));
    let id = match syscall::Id::from_raw(id_raw) {
        Some(id) => id,
        None => return Errno::Einval as i32 as u32,
    };
    // Args en r0–r3 del frame (offset 0,4,8,12).
    let frame = psp as *mut u32;
    // SAFETY: frame válido creado por hardware en SVC entry.
    let args = unsafe {
        [
            ptr::read_volatile(frame),
            ptr::read_volatile(frame.add(1)),
            ptr::read_volatile(frame.add(2)),
            ptr::read_volatile(frame.add(3)),
        ]
    };
    let ret = syscall::dispatch(id, args);
    // Escribir retorno en r0 del frame.
    // SAFETY: mismo frame.
    unsafe {
        ptr::write_volatile(frame, ret as u32);
    }
    ret as u32
}

global_asm!(
    ".syntax unified",
    ".global SVCall",
    ".thumb_func",
    "SVCall:",
    "  mrs r0, psp",
    "  push {{r4, lr}}",
    "  bl rugus_svc_handler",
    "  pop {{r4, lr}}",
    "  bx lr",
);
