//! Tabla de vectores de excepción EL1 y handler de IRQ del backend AArch64.
//!
//! Contraparte del PendSV/SysTick de Cortex-M: aquí vive la tabla `VBAR_EL1` y
//! el handler de IRQ que convierte el latido del [`crate::time`] Generic Timer
//! en **preempción del scheduler de `rugus-core`**.
//!
//! ## Cómo preempta sin PendSV
//!
//! En Cortex-M el cambio de contexto en preempción lo difiere el PendSV. AArch64
//! no tiene PendSV, pero no hace falta: el handler de IRQ salva el contexto
//! **completo** de la tarea interrumpida (x0–x30 + ELR/SPSR) en su pila y llama
//! a [`rust_irq`], que atiende el timer y dispara el hook de preempción
//! (`preempt_tick` → `switch_context`). Ese `switch_context` es el `cpu_switch`
//! cooperativo (frame de callee-saved de 160 B): se ejecuta **anidado** dentro
//! del frame de excepción y conmuta a otra tarea. Cuando el scheduler vuelve a
//! elegir esta tarea, `cpu_switch` retorna aquí, el handler restaura su frame de
//! excepción y hace `eret`. El scheduler solo conmuta entre frames `cpu_switch`;
//! el frame de excepción es transparente para él.
//!
//! ## Alcance del guardado
//!
//! Se salvan los GPR enteros (x0–x30) + ELR_EL1/SPSR_EL1, igual que el scheduler
//! preemptivo de referencia validado en HW (`examples/rpi3-sched`). Los NEON
//! callee-saved (d8–d15) los preserva `cpu_switch`; los NEON caller-saved
//! (d0–d7, d16–d31) NO se salvan: el camino del scheduler de `rugus-core` es
//! entero. Si en el futuro corren tareas con FP intensivo, ampliar el frame.

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Frame de excepción: x0..x30 (31×8 = 248 B) + ELR_EL1 + SPSR_EL1 = 264 B,
// redondeado a 272 B para alineación a 16. El handler (asm) usa estos offsets.

// Tabla de vectores EL1 + handlers. La tabla va alineada a 2 KiB (`.align 11`)
// y cada una de las 16 entradas a 128 B (`.align 7`), como exige ARMv8-A.
global_asm!(
    r#"
.section ".text"
.align 11
.global rugus_vector_table
rugus_vector_table:
    // --- Current EL con SP_EL0 (no usado: Rugus corre en EL1h/SP_EL1) ---
    .align 7
    b       rugus_hang_exc          // Sync
    .align 7
    b       rugus_hang_exc          // IRQ
    .align 7
    b       rugus_hang_exc          // FIQ
    .align 7
    b       rugus_hang_exc          // SError
    // --- Current EL con SP_ELx (EL1h): aquí entran las excepciones del kernel ---
    .align 7
    b       rugus_el1_sync          // Sync
    .align 7
    b       rugus_el1_irq           // IRQ  ← preempción
    .align 7
    b       rugus_hang_exc          // FIQ
    .align 7
    b       rugus_hang_exc          // SError
    // --- Lower EL AArch64 (EL0): sin userland EL0 todavía ---
    .align 7
    b       rugus_hang_exc
    .align 7
    b       rugus_hang_exc
    .align 7
    b       rugus_hang_exc
    .align 7
    b       rugus_hang_exc
    // --- Lower EL AArch32: no soportado ---
    .align 7
    b       rugus_hang_exc
    .align 7
    b       rugus_hang_exc
    .align 7
    b       rugus_hang_exc
    .align 7
    b       rugus_hang_exc

// IRQ EL1h: salva el contexto completo de la tarea interrumpida, llama a
// `rust_irq` (que puede conmutar de tarea de forma anidada) y, al volver,
// restaura el contexto y hace `eret`.
rugus_el1_irq:
    sub     sp, sp, #272
    stp     x0,  x1,  [sp, #16 * 0]
    stp     x2,  x3,  [sp, #16 * 1]
    stp     x4,  x5,  [sp, #16 * 2]
    stp     x6,  x7,  [sp, #16 * 3]
    stp     x8,  x9,  [sp, #16 * 4]
    stp     x10, x11, [sp, #16 * 5]
    stp     x12, x13, [sp, #16 * 6]
    stp     x14, x15, [sp, #16 * 7]
    stp     x16, x17, [sp, #16 * 8]
    stp     x18, x19, [sp, #16 * 9]
    stp     x20, x21, [sp, #16 * 10]
    stp     x22, x23, [sp, #16 * 11]
    stp     x24, x25, [sp, #16 * 12]
    stp     x26, x27, [sp, #16 * 13]
    stp     x28, x29, [sp, #16 * 14]
    str     x30,      [sp, #240]
    mrs     x9,  elr_el1
    mrs     x10, spsr_el1
    stp     x9,  x10, [sp, #256]

    bl      rust_irq                 // atiende timer + preempción (anidada)

    ldp     x9,  x10, [sp, #256]
    msr     elr_el1,  x9
    msr     spsr_el1, x10
    ldp     x0,  x1,  [sp, #16 * 0]
    ldp     x2,  x3,  [sp, #16 * 1]
    ldp     x4,  x5,  [sp, #16 * 2]
    ldp     x6,  x7,  [sp, #16 * 3]
    ldp     x8,  x9,  [sp, #16 * 4]
    ldp     x10, x11, [sp, #16 * 5]
    ldp     x12, x13, [sp, #16 * 6]
    ldp     x14, x15, [sp, #16 * 7]
    ldp     x16, x17, [sp, #16 * 8]
    ldp     x18, x19, [sp, #16 * 9]
    ldp     x20, x21, [sp, #16 * 10]
    ldp     x22, x23, [sp, #16 * 11]
    ldp     x24, x25, [sp, #16 * 12]
    ldp     x26, x27, [sp, #16 * 13]
    ldp     x28, x29, [sp, #16 * 14]
    ldr     x30,      [sp, #240]
    add     sp, sp, #272
    eret

// Sync EL1: captura ESR/ELR y rutea a Rust (post-mortem mínimo).
rugus_el1_sync:
    mrs     x0, esr_el1
    mrs     x1, elr_el1
    bl      rust_sync
1:  wfe
    b       1b

rugus_hang_exc:
    bl      rust_unexpected_exc
1:  wfe
    b       1b
"#
);

/// Hook de fault síncrono opcional (`fn(esr, elr)`), registrable por el kernel.
static SYNC_HOOK: AtomicUsize = AtomicUsize::new(0);
/// Hook de IRQ de periférico opcional (`fn()`), p.ej. drenar el RX del UART a un
/// ring. Lo llama [`rust_irq`] **después** del Generic Timer.
static IRQ_HOOK: AtomicUsize = AtomicUsize::new(0);
/// `true` si alguna capa pidió arrancar la primera tarea con IRQs habilitadas
/// aunque no haya preempción por timer (p.ej. una consola con RX por IRQ).
static WANT_IRQS: AtomicBool = AtomicBool::new(false);

/// Registra un hook de IRQ de periférico (`fn()`), invocado en cada IRQ tras
/// atender el Generic Timer. Pensado para RX por interrupción (UART→ring).
pub fn set_irq_hook(hook: fn()) {
    IRQ_HOOK.store(hook as usize, Ordering::Relaxed);
}

/// Pide que la primera tarea arranque con IRQs habilitadas aunque no haya
/// preempción por timer (consola con RX por IRQ). Lo consulta `start_first`.
pub fn request_irqs_at_start() {
    WANT_IRQS.store(true, Ordering::Relaxed);
}

/// `true` si se solicitó habilitar IRQs al entrar en la primera tarea.
#[inline]
pub fn irqs_requested() -> bool {
    WANT_IRQS.load(Ordering::Relaxed)
}

/// Registra un manejador para excepciones síncronas EL1 (`fn(esr, elr)`).
///
/// Sin hook, [`rust_sync`] simplemente cuelga el core (post-mortem por UART lo
/// pone la capa superior). Permite a la capa de kernel enganchar telemetría de
/// faults sin que el arch dependa de ella.
pub fn set_sync_hook(hook: fn(u64, u64)) {
    SYNC_HOOK.store(hook as usize, Ordering::Relaxed);
}

/// Instala la tabla de vectores de Rugus en `VBAR_EL1`.
///
/// Debe llamarse tras el arranque (EL2→EL1) y antes de habilitar IRQs. La pila
/// EL1 ya debe estar fijada (las excepciones apilan sobre `SP_EL1`).
pub fn install() {
    extern "C" {
        static rugus_vector_table: u8;
    }
    // SAFETY: `rugus_vector_table` está alineada a 2 KiB (la tabla del global_asm)
    // y es válida para todo el tiempo de vida del kernel.
    unsafe {
        let vbar = core::ptr::addr_of!(rugus_vector_table) as u64;
        core::arch::asm!("msr vbar_el1, {}", "isb", in(reg) vbar);
    }
}

/// Trampolín de IRQ llamado desde el handler asm: atiende el Generic Timer y,
/// si venció el quantum, dispara la preempción (cambio de contexto anidado).
#[no_mangle]
extern "C" fn rust_irq() {
    // Primero el quantum de preempción (inofensivo si no hay timer: comprueba la
    // fuente y retorna). Luego el hook de periférico (RX del UART, etc.).
    crate::time::on_irq();
    let hook = IRQ_HOOK.load(Ordering::Relaxed);
    if hook != 0 {
        // SAFETY: solo se escribe en `set_irq_hook` con un `fn()` válido.
        let f: fn() = unsafe { core::mem::transmute(hook) };
        f();
    }
}

/// Trampolín de excepción síncrona: invoca el hook registrado si lo hay.
#[no_mangle]
extern "C" fn rust_sync(esr: u64, elr: u64) {
    let hook = SYNC_HOOK.load(Ordering::Relaxed);
    if hook != 0 {
        // SAFETY: solo se escribe en `set_sync_hook` con un `fn(u64,u64)` válido.
        let f: fn(u64, u64) = unsafe { core::mem::transmute(hook) };
        f(esr, elr);
    }
}

/// Trampolín de excepción inesperada (FIQ/SError/EL0): no-op; el handler cuelga.
#[no_mangle]
extern "C" fn rust_unexpected_exc() {}
