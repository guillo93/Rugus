//! Context switch vía PendSV — Cortex-M3/M4/M7, privilegio user/priv y FPU.
//!
//! # Frame de contexto
//!
//! El estado de una tarea suspendida vive en su propio stack (PSP). De arriba
//! (direcciones altas) hacia abajo:
//!
//! 1. **Frame hardware** (8 words): `xPSR, PC, LR, R12, R3, R2, R1, R0`. Lo
//!    apila/desapila el propio mecanismo de excepción del Cortex-M. Si la tarea
//!    tenía estado FP activo (`CONTROL.FPCA`), el hardware extiende este frame
//!    con `S0–S15 + FPSCR` automáticamente (lazy stacking, `FPCCR.ASPEN=1`).
//! 2. **Frame software** (9 words): `R4–R11` + `EXC_RETURN`. Lo guarda/restaura
//!    este módulo, porque son registros *callee-saved* que el frame hardware no
//!    cubre. Persistir `EXC_RETURN` por tarea es lo que permite que cada una
//!    recuerde si traía estado FP (bit 4) y privilegio de retorno.
//! 3. **Frame FP callee-saved** (`S16–S31`, solo si `EXC_RETURN` bit4 == 0):
//!    también callee-saved y tampoco cubierto por el frame hardware. Solo se
//!    guarda/restaura en targets con FPU (`eabihf`) y solo si la tarea traía
//!    estado FP. En M3 (`eabi`, sin FPU) esas instrucciones VFP ni se emiten.
//!
//! Esta simetría (push S16–S31 → push {r4-r11,lr}; pop {r4-r11,lr} → pop
//! S16–S31) mantiene el mismo orden de direcciones en save y restore.

use crate::Context;
use core::arch::global_asm;
use core::ptr;

/// Puntero al contexto previo (visible al ASM).
#[no_mangle]
static mut RUGUS_SWITCH_PREV: *mut Context = ptr::null_mut();

/// Puntero al contexto siguiente (visible al ASM).
#[no_mangle]
static mut RUGUS_SWITCH_NEXT: *const Context = ptr::null();

/// EXC_RETURN para tarea en thread mode sobre PSP **sin** estado FP (frame
/// básico). Bit4 = 1 → el hardware no apila/desapila S0–S15.
const EXC_RETURN_THREAD_PSP: u32 = 0xFFFF_FFFD;

/// Palabras del frame software: `r4–r11` (8) + `EXC_RETURN` (1).
const SW_FRAME_WORDS: usize = 9;
/// Palabras del frame hardware básico: `r0–r3, r12, lr, pc, xpsr`.
const HW_FRAME_WORDS: usize = 8;

/// Prepara el stack inicial de una tarea nueva (sin estado FP).
pub fn init_task_stack(stack: &mut [u8], entry: fn() -> !, privileged: bool) -> Context {
    let words = stack.len() / 4;
    assert!(words >= 64, "stack too small");
    let base = stack.as_mut_ptr() as *mut u32;
    // SAFETY: stack exclusivo de la tarea, alineado a 4 bytes, words >= 64.
    unsafe {
        // Frame hardware en [words-8 .. words-1].
        *base.add(words - 1) = 0x0100_0000; // xPSR (Thumb)
        *base.add(words - 2) = (entry as usize as u32) | 1; // PC
        *base.add(words - 3) = EXC_RETURN_THREAD_PSP; // LR (centinela; entry es -> !)
        *base.add(words - 4) = 0; // r12
        *base.add(words - 5) = 0; // r3
        *base.add(words - 6) = 0; // r2
        *base.add(words - 7) = 0; // r1
        *base.add(words - 8) = 0; // r0
                                  // Frame software en [words-17 .. words-9]: r4..r11 (bajo→alto) + EXC_RETURN.
        *base.add(words - 9) = EXC_RETURN_THREAD_PSP; // EXC_RETURN (word más alto del SW frame)
        for i in 0..8 {
            *base.add(words - 10 - i) = 0; // r11..r4
        }
        let sp = base.add(words - HW_FRAME_WORDS - SW_FRAME_WORDS) as usize as u32;
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
/// No es un retorno de excepción: fija PSP por encima del frame hardware y salta
/// directo a `entry`. Solo válido para una tarea recién creada por
/// [`init_task_stack`] (frame básico, sin estado FP).
///
/// # Safety
///
/// `ctx` debe apuntar a un [`Context`] con stack inicializado por
/// [`init_task_stack`].
pub unsafe fn restore_context(ctx: *const Context) -> ! {
    unsafe {
        let sp = (*ctx).sp;
        let frame = sp as *const u32;
        // PC está en el word (words-2); respecto a sp (= words-17) → +15.
        let entry = frame.add(SW_FRAME_WORDS + 6).read();
        // PSP por encima del frame hardware: sp + (9 sw + ... ) → inicio del HW frame.
        cortex_m::register::psp::write(sp + (SW_FRAME_WORDS as u32) * 4);
        let ctrl = control_for_context(ctx);
        cortex_m::register::control::write(cortex_m::register::control::Control::from_bits(ctrl));
        cortex_m::asm::isb();
        core::arch::asm!("bx {}", in(reg) entry, options(noreturn));
    }
}

/// Reanuda `ctx` tras matar la tarea faultante, reutilizando el restore de
/// PendSV (path de kill+resume).
///
/// En vez de hacer un retorno de excepción a medida (que tendría que replicar
/// — y mantener en sincronía — la lógica FP del restore de PendSV, incluido el
/// bookkeeping de *lazy stacking* del Cortex-M), arma el switch hacia `ctx` con
/// `RUGUS_SWITCH_PREV = null` (no hay contexto saliente que guardar) y pende
/// PendSV. PendSV tiene la prioridad más baja, así que al salir de este fault
/// handler hace **tail-chain** inmediato y ejecuta su secuencia de restore ya
/// probada (FP-aware). El frame que el hardware desapilaría en este retorno NO
/// se usa: el tail-chain ocurre antes de ejecutar instrucción alguna de thread.
///
/// # Safety
///
/// `ctx` debe apuntar a un [`Context`] válido guardado por un switch previo.
pub unsafe fn resume_after_fault(ctx: *const Context) -> ! {
    unsafe {
        RUGUS_SWITCH_PREV = ptr::null_mut();
        RUGUS_SWITCH_NEXT = ctx;
        pend_pendsv();
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        // EXC_RETURN a thread/PSP (frame básico). Su valor exacto es indiferente:
        // PendSV hace tail-chain antes de desapilar y, al restaurar `ctx`, fija
        // PSP/CONTROL y retorna con el EXC_RETURN propio de la tarea.
        core::arch::asm!(
            "movw lr, #0xFFFD",
            "movt lr, #0xFFFF",
            "bx lr",
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

// PendSV: guarda el contexto saliente y restaura el entrante. En targets con
// FPU (`eabihf`) preserva además S16–S31 condicional a EXC_RETURN bit4. En M3
// (`eabi`) esas instrucciones VFP no existen, así que se compila sin ellas; el
// EXC_RETURN de toda tarea M3 es constante (0xFFFFFFFD), de modo que el `bx lr`
// final equivale al comportamiento previo.
#[cfg(target_abi = "eabihf")]
global_asm!(
    ".syntax unified",
    ".fpu vfpv4",
    ".global PendSV",
    ".thumb_func",
    "PendSV:",
    "  ldr r1, =RUGUS_SWITCH_PREV",
    "  ldr r2, [r1]",
    "  cbz r2, 2f",
    "  mrs r0, psp",
    "  tst lr, #0x10",
    "  it eq",
    "  vstmdbeq r0!, {{s16-s31}}",
    "  stmdb r0!, {{r4-r11, lr}}",
    "  str r0, [r2]",
    "2:",
    "  ldr r1, =RUGUS_SWITCH_NEXT",
    "  ldr r1, [r1]",
    "  cbz r1, 3f",
    "  ldr r0, [r1]",
    "  ldmia r0!, {{r4-r11, lr}}",
    "  tst lr, #0x10",
    "  it eq",
    "  vldmiaeq r0!, {{s16-s31}}",
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

#[cfg(not(target_abi = "eabihf"))]
global_asm!(
    ".syntax unified",
    ".global PendSV",
    ".thumb_func",
    "PendSV:",
    "  ldr r1, =RUGUS_SWITCH_PREV",
    "  ldr r2, [r1]",
    "  cbz r2, 2f",
    "  mrs r0, psp",
    "  stmdb r0!, {{r4-r11, lr}}",
    "  str r0, [r2]",
    "2:",
    "  ldr r1, =RUGUS_SWITCH_NEXT",
    "  ldr r1, [r1]",
    "  cbz r1, 3f",
    "  ldr r0, [r1]",
    "  ldmia r0!, {{r4-r11, lr}}",
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
