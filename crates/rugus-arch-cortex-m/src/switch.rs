//! Context switch vía PendSV — Cortex-M7.

use crate::Context;
use core::arch::global_asm;
use core::ptr;

/// Puntero al contexto previo (visible al ASM).
#[no_mangle]
static mut RUGUS_SWITCH_PREV: *mut Context = ptr::null_mut();

/// Puntero al contexto siguiente (visible al ASM).
#[no_mangle]
static mut RUGUS_SWITCH_NEXT: *const Context = ptr::null();

/// Prepara el stack inicial de una tarea nueva.
pub fn init_task_stack(stack: &mut [u8], entry: fn() -> !) -> Context {
    let words = stack.len() / 4;
    assert!(words >= 64, "stack too small");
    let base = stack.as_mut_ptr() as *mut u32;
    // SAFETY: stack exclusivo de la tarea, alineado a 4 bytes.
    unsafe {
        let top = words - 1;
        *base.add(top) = 0x0100_0000; // xPSR (Thumb, align)
        *base.add(top - 1) = (entry as usize as u32) | 1; // PC
        *base.add(top - 2) = 0xFFFF_FFFD; // LR (EXC_RETURN, thread/PSP, no FPU)
        *base.add(top - 3) = 0;
        *base.add(top - 4) = 0;
        *base.add(top - 5) = 0;
        *base.add(top - 6) = 0;
        *base.add(top - 7) = 0;
        for i in 0..8 {
            *base.add(top - 8 - i) = 0;
        }
        let sp = base.add(top - 15) as usize as u32;
        Context { sp }
    }
}

/// Arranca la primera tarea; no retorna.
pub fn start_first(ctx: *const Context) -> ! {
    configure_pendsv_priority();
    // SAFETY: ctx válido; PC con bit Thumb en el frame sintético.
    unsafe {
        let frame = (*ctx).sp as *const u32;
        let entry = frame.add(14).read();
        cortex_m::register::psp::write((*ctx).sp + 32);
        let mut ctrl = cortex_m::register::control::read();
        ctrl.set_spsel(cortex_m::register::control::Spsel::Psp);
        cortex_m::register::control::write(ctrl);
        core::arch::asm!("bx {}", in(reg) entry, options(noreturn));
    }
}

/// Solicita cambio de contexto cooperativo (PendSV).
///
/// # Safety
///
/// `prev` y `next` deben apuntar a [`Context`] válidos con stacks inicializados.
pub unsafe fn request_switch(prev: *mut Context, next: *const Context) {
    unsafe {
        RUGUS_SWITCH_PREV = prev;
        RUGUS_SWITCH_NEXT = next;
        pend_pendsv();
        cortex_m::asm::isb();
    }
}

fn configure_pendsv_priority() {
    use cortex_m::peripheral::scb::SystemHandler;
    unsafe {
        let mut scb = cortex_m::Peripherals::steal().SCB;
        scb.set_priority(SystemHandler::PendSV, 0xFF);
    }
}

fn pend_pendsv() {
    const ICSR: *mut u32 = 0xE000_ED04 as *mut u32;
    const PENDSVSET: u32 = 1 << 28;
    unsafe {
        let val = ptr::read_volatile(ICSR);
        ptr::write_volatile(ICSR, val | PENDSVSET);
    }
}

global_asm!(
    ".syntax unified",
    ".global PendSV",
    ".thumb_func",
    "PendSV:",
    "  mrs r0, psp",
    "  cbz r0, 1f",
    "  stmdb r0!, {{r4-r11}}",
    "  mov r2, r0",
    "  ldr r1, =RUGUS_SWITCH_PREV",
    "  ldr r1, [r1]",
    "  cbz r1, 1f",
    "  str r2, [r1]",
    "1:",
    "  ldr r1, =RUGUS_SWITCH_NEXT",
    "  ldr r1, [r1]",
    "  cbz r1, 3f",
    "  ldr r0, [r1]",
    "  ldmia r0!, {{r4-r11}}",
    "  msr psp, r0",
    "  mov r0, #2",
    "  msr control, r0",
    "  isb",
    "3:",
    "  movs r0, #0",
    "  ldr r1, =RUGUS_SWITCH_PREV",
    "  str r0, [r1]",
    "  ldr r1, =RUGUS_SWITCH_NEXT",
    "  str r0, [r1]",
    "  bx lr",
);
