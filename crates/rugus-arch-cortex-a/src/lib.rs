//! Backend `Arch` de Rugus para **Cortex-A / ARMv8-A (AArch64)**.
//!
//! Implementa el contrato [`rugus_core::arch::Arch`] sobre AArch64, de modo que
//! el **mismo scheduler/kernel arch-agnóstico** de `rugus-core` corra en una
//! Raspberry Pi 3B+ igual que en los Cortex-M, sin tocar su lógica. Es la
//! contraparte de `rugus-arch-cortex-m`.
//!
//! ## Modelo
//!
//! - **Context** = el `SP` de la tarea, que apunta a su frame de registros
//!   *callee-saved* guardado en su propia pila (x19–x30 + d8–d15, AAPCS64).
//! - **Cambio cooperativo** ([`switch_context`]): guarda los callee-saved de la
//!   tarea saliente, conmuta `SP` y restaura los de la entrante; `ret` reanuda.
//! - **Secciones críticas**: máscara de IRQ vía `DAIF`.
//! - **Reloj**: Generic Timer (`CNTPCT_EL0`/`CNTFRQ_EL0`).
//! - Sin MPU per-tarea todavía (`HAS_MEMORY_PROTECTION = false`); la protección
//!   por MMU/EL0 llega en una capa posterior.

#![no_std]

use core::arch::global_asm;
use core::ptr::write_volatile;
use rugus_core::arch::{Arch, CriticalGuard};
use rugus_core::sched::TaskMode;

/// Backend AArch64.
pub struct CortexA;

/// Contexto de tarea: el `SP` apunta al frame de callee-saved en su pila.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Context {
    /// Stack pointer guardado (frame de x19–x30 + d8–d15).
    pub sp: u64,
}

/// Tamaño del frame de callee-saved (12 GPR + 8 FP/SIMD), alineado a 16.
const CTX_FRAME: usize = 160;

/// Handle de sección crítica: valor previo de `DAIF`.
pub struct SavedDaif(u64);
impl CriticalGuard for SavedDaif {}

// Cambio de contexto cooperativo y arranque de la primera tarea, en ASM.
// `cpu_switch(prev: *mut Context, next: *const Context)` y
// `cpu_start_first(ctx: *const Context) -> !`. El frame (160 B) lo comparten
// con `init_task_stack` (abajo) — deben cuadrar exactamente.
global_asm!(
    r#"
.global cpu_switch
cpu_switch:
    sub     sp, sp, #160
    stp     x19, x20, [sp, #0]
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    stp     d8,  d9,  [sp, #96]
    stp     d10, d11, [sp, #112]
    stp     d12, d13, [sp, #128]
    stp     d14, d15, [sp, #144]
    mov     x2, sp
    str     x2, [x0]              // prev->sp = sp actual
    ldr     x2, [x1]              // sp = next->sp
    mov     sp, x2
    b       cpu_restore

.global cpu_start_first
cpu_start_first:
    ldr     x2, [x0]              // sp = ctx->sp
    mov     sp, x2
cpu_restore:
    ldp     x19, x20, [sp, #0]
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    ldp     d8,  d9,  [sp, #96]
    ldp     d10, d11, [sp, #112]
    ldp     d12, d13, [sp, #128]
    ldp     d14, d15, [sp, #144]
    add     sp, sp, #160
    ret                          // salta a x30 (lr): reanuda o entra en `entry`
"#
);

extern "C" {
    fn cpu_switch(prev: *mut Context, next: *const Context);
    fn cpu_start_first(ctx: *const Context) -> !;
}

// Registros del watchdog del BCM2837 para `reset()` (RPi 3).
const PM_RSTC: usize = 0x3F10_001C;
const PM_WDOG: usize = 0x3F10_0024;
const PM_PASSWORD: u32 = 0x5A00_0000;
const PM_RSTC_FULLRST: u32 = 0x20;

impl Arch for CortexA {
    type Context = Context;
    type SavedIrq = SavedDaif;

    const HAS_MEMORY_PROTECTION: bool = false;

    unsafe fn switch_context(prev: *mut Self::Context, next: *const Self::Context) {
        // SAFETY: `prev`/`next` son Contexts válidos del scheduler.
        unsafe { cpu_switch(prev, next) }
    }

    fn init_task_stack(stack: &mut [u8], entry: fn() -> !, _privileged: bool) -> Self::Context {
        // Frame inicial en el tope de la pila (alineado a 16): callee-saved a 0
        // y x30 (lr) = `entry`, de modo que `cpu_restore`+`ret` salte a la tarea.
        let top = ((stack.as_ptr() as usize) + stack.len()) & !0xF;
        let sp = top - CTX_FRAME;
        let f = sp as *mut u64;
        // SAFETY: [sp, sp+160) cae dentro de la pila estática de la tarea.
        unsafe {
            for i in 0..(CTX_FRAME / 8) {
                write_volatile(f.add(i), 0);
            }
            write_volatile(f.add(11), entry as usize as u64); // offset 88 → x30 (lr)
        }
        Context { sp: sp as u64 }
    }

    fn start_first(ctx: *const Self::Context) -> ! {
        // SAFETY: primer arranque; `ctx` válido del scheduler.
        unsafe { cpu_start_first(ctx) }
    }

    unsafe fn resume_after_fault(ctx: *const Self::Context) -> ! {
        // Sin contención de faults por tarea todavía: reanudar = arrancar el ctx.
        // SAFETY: `ctx` válido; el scheduler eligió la tarea a reanudar.
        unsafe { cpu_start_first(ctx) }
    }

    fn on_task_switch(_mode: TaskMode, _stack_base: u32, _stack_len: u32) {
        // No-op: sin MPU/EL0 per-tarea en esta capa.
    }

    fn enter_critical() -> Self::SavedIrq {
        let daif: u64;
        // SAFETY: lectura de DAIF + máscara de IRQ (DAIFSet.I).
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif);
            core::arch::asm!("msr daifset, #2");
        }
        SavedDaif(daif)
    }

    fn exit_critical(saved: Self::SavedIrq) {
        // DAIF bit I = bit 7: 0 ⇒ IRQ estaban habilitadas antes. Solo re-habilita
        // en ese caso (respeta secciones críticas anidadas).
        if saved.0 & (1 << 7) == 0 {
            // SAFETY: restaura la máscara de IRQ previa.
            unsafe { core::arch::asm!("msr daifclr, #2") };
        }
    }

    fn wait_for_interrupt() {
        // SAFETY: instrucción de espera; sin efectos sobre memoria.
        unsafe { core::arch::asm!("wfi") };
    }

    fn now_ms() -> u32 {
        let cnt: u64;
        let frq: u64;
        // SAFETY: lectura de registros del Generic Timer (solo lectura).
        unsafe {
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt);
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq);
        }
        let per_ms = (frq / 1000).max(1);
        (cnt / per_ms) as u32
    }

    fn reset() -> ! {
        // Reset por el watchdog del BCM2837 (RPi 3).
        // SAFETY: registros del power manager; escritura única de reset.
        unsafe {
            write_volatile(PM_WDOG as *mut u32, PM_PASSWORD | 1);
            write_volatile(PM_RSTC as *mut u32, PM_PASSWORD | PM_RSTC_FULLRST);
        }
        loop {
            // SAFETY: espera al reset inminente.
            unsafe { core::arch::asm!("wfe") };
        }
    }
}
