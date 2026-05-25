//! Context switch vía PendSV — Cortex-M7 + privilegio user/priv.

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
pub fn init_task_stack(stack: &mut [u8], entry: fn() -> !, privileged: bool) -> Context {
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
        Context {
            sp,
            privileged: if privileged { 1 } else { 0 },
        }
    }
}

fn control_for_context(ctx: *const Context) -> u32 {
    // CONTROL: bit1=SPSEL (PSP), bit0=nPRIV (0=priv, 1=user).
    // privileged=1 → 2; privileged=0 → 3.
    let priv_flag = unsafe { (*ctx).privileged };
    2 + (1 - priv_flag)
}

/// Arranca la primera tarea; no retorna.
pub fn start_first(ctx: *const Context) -> ! {
    configure_pendsv_priority();
    // SAFETY: ctx válido; PC con bit Thumb en el frame sintético.
    unsafe {
        restore_context(ctx);
    }
}

/// Restaura `ctx` y retorna a thread mode (bootstrap desde `main`).
///
/// # Safety
///
/// `ctx` debe apuntar a un [`Context`] con stack inicializado.
pub unsafe fn restore_context(ctx: *const Context) -> ! {
    unsafe {
        let frame = (*ctx).sp as *const u32;
        let entry = frame.add(14).read();
        cortex_m::register::psp::write((*ctx).sp + 32);
        let ctrl = control_for_context(ctx);
        cortex_m::register::control::write(cortex_m::register::control::Control::from_bits(ctrl));
        cortex_m::asm::isb();
        core::arch::asm!("bx {}", in(reg) entry, options(noreturn));
    }
}

/// Restaura `ctx` y sale del fault handler vía EXC_RETURN (PendSV/fault path).
///
/// # Safety
///
/// `ctx` debe apuntar a un [`Context`] con stack inicializado.
pub unsafe fn resume_after_fault(ctx: *const Context) -> ! {
    unsafe {
        let sp = (*ctx).sp;
        let priv_word = (*ctx).privileged;
        core::arch::asm!(
            "ldmia {sp}, {{r4-r11}}",
            "adds r12, {sp}, #32",
            "msr psp, r12",
            "rsbs r3, {priv}, #3",
            "msr control, r3",
            "isb",
            "mov lr, #0xFFFFFFFD",
            "bx lr",
            sp = in(reg) sp,
            priv = in(reg) priv_word,
            options(noreturn)
        );
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
    "  ldr r3, [r1, #4]",
    "  rsbs r3, r3, #3",
    "  msr control, r3",
    "  isb",
    "3:",
    "  movs r0, #0",
    "  ldr r1, =RUGUS_SWITCH_PREV",
    "  str r0, [r1]",
    "  ldr r1, =RUGUS_SWITCH_NEXT",
    "  str r0, [r1]",
    "  bx lr",
);
