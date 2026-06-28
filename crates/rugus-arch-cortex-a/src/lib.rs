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
// El contrato `Arch` define `start_first`/`resume_after_fault` como `fn` que
// reciben `*const Context` (el scheduler garantiza su validez); el ABI por
// puntero es inherente al backend. El lint no aplica a este patrón.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod time;
pub mod vectors;

use core::arch::global_asm;
use core::ptr::{addr_of, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};
use rugus_core::arch::{Arch, CriticalGuard};
use rugus_core::sched::TaskMode;

// Pila de kernel por tarea userland (EL0): el frame de excepción de una tarea
// EL0 NO puede vivir en su pila de usuario (EL0-accesible → podría manipular su
// propio SPSR y escalar a EL1). Vive en una pila kernel (EL1-only) de este pool.
// El `stack` que el scheduler pasa para una tarea EL0 es su pila de USUARIO
// (SP_EL0). Soporta hasta `N_USER_TASKS` tareas EL0.
const N_USER_TASKS: usize = 4;
const KSTACK_SZ: usize = 4096;
#[repr(C, align(16))]
struct KStack([u8; KSTACK_SZ]);
static mut KERNEL_STACKS: [KStack; N_USER_TASKS] =
    [const { KStack([0; KSTACK_SZ]) }; N_USER_TASKS];
static KSTACK_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Reserva el tope de una pila kernel del pool para una tarea EL0. Aborta (vía
/// reset) si se agota el pool — es un error de configuración del firmware.
fn alloc_kernel_stack_top() -> u64 {
    let i = KSTACK_NEXT.fetch_add(1, Ordering::Relaxed);
    if i >= N_USER_TASKS {
        CortexA::reset();
    }
    // SAFETY: `i < N_USER_TASKS`; dirección de un elemento del pool estático.
    let base = unsafe { addr_of!(KERNEL_STACKS[i]) } as u64;
    (base + KSTACK_SZ as u64) & !0xF
}

/// Backend AArch64.
pub struct CortexA;

/// Contexto de tarea: el `SP` apunta al frame de callee-saved en su pila.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Context {
    /// Stack pointer guardado (frame de x19–x30 + d8–d15).
    pub sp: u64,
}

/// Tamaño del **frame de excepción unificado** (G6.2): x0..x30 [0..248],
/// SP_EL0 [248], ELR_EL1 [256], SPSR_EL1 [264], d8..d15 [272..336]. Alineado a
/// 16. Lo comparten `cpu_switch`/`cpu_start_first`/`init_task_stack` y es el
/// formato que también usarán las tareas EL0 (mismo frame, se reanuda con
/// `eret`). Ver `docs/AARCH64-USERLAND-DESIGN.md`.
const CTX_FRAME: usize = 336;

/// Handle de sección crítica: valor previo de `DAIF`.
pub struct SavedDaif(u64);
impl CriticalGuard for SavedDaif {}

// Cambio de contexto y arranque de la primera tarea, en ASM, sobre el frame
// unificado (336 B). `cpu_switch` (cooperativo) sintetiza un frame de excepción
// del hilo que cede: guarda los callee-saved (los caller-saved están muertos en
// un límite de llamada AAPCS), `ELR=lr`, `SPSR=EL1h | DAIF actual` (preserva la
// máscara de IRQ de la sección crítica en curso) y `SP_EL0` actual. La
// reanudación es **siempre `eret`**: válido EL1h→EL1h y, en el futuro, hacia
// EL0t. El handler de IRQ (vectors.rs) sigue anidando este `cpu_switch` igual.
global_asm!(
    r#"
.global cpu_switch
cpu_switch:
    sub     sp, sp, #336
    stp     x19, x20, [sp, #152]      // callee-saved GPR en sus huecos
    stp     x21, x22, [sp, #168]
    stp     x23, x24, [sp, #184]
    stp     x25, x26, [sp, #200]
    stp     x27, x28, [sp, #216]
    stp     x29, x30, [sp, #232]
    mrs     x2,  sp_el0
    str     x2,  [sp, #248]
    str     x30, [sp, #256]          // ELR = lr → reanuda tras la llamada
    mrs     x2,  daif
    mov     x3,  #0x5               // M=EL1h (x3 caller-saved, muerto aquí)
    orr     x2,  x2, x3             // SPSR: M=EL1h, DAIF = máscara actual
    str     x2,  [sp, #264]
    stp     d8,  d9,  [sp, #272]
    stp     d10, d11, [sp, #288]
    stp     d12, d13, [sp, #304]
    stp     d14, d15, [sp, #320]
    mov     x2, sp
    str     x2, [x0]                  // prev->sp = sp actual
    ldr     x2, [x1]                  // sp = next->sp
    mov     sp, x2
    b       cpu_restore

.global cpu_start_first
cpu_start_first:
    ldr     x2, [x0]                  // sp = ctx->sp
    mov     sp, x2
cpu_restore:
    ldr     x2,  [sp, #264]
    msr     spsr_el1, x2
    ldr     x2,  [sp, #256]
    msr     elr_el1,  x2
    ldr     x2,  [sp, #248]
    msr     sp_el0,   x2
    ldp     x0,  x1,  [sp, #0]
    ldp     x2,  x3,  [sp, #16]
    ldp     x4,  x5,  [sp, #32]
    ldp     x6,  x7,  [sp, #48]
    ldp     x8,  x9,  [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldr     x30,      [sp, #240]
    ldp     d8,  d9,  [sp, #272]
    ldp     d10, d11, [sp, #288]
    ldp     d12, d13, [sp, #304]
    ldp     d14, d15, [sp, #320]
    add     sp, sp, #336
    eret                             // reanuda EL1h (o, futuro, EL0t)
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

    const HAS_MEMORY_PROTECTION: bool = true;

    unsafe fn switch_context(prev: *mut Self::Context, next: *const Self::Context) {
        // SAFETY: `prev`/`next` son Contexts válidos del scheduler.
        unsafe { cpu_switch(prev, next) }
    }

    fn init_task_stack(stack: &mut [u8], entry: fn() -> !, privileged: bool) -> Self::Context {
        // El frame de excepción inicial (336 B) determina el EL de la tarea por
        // su `SPSR`. La reanudación (`cpu_restore`+`eret`) entra en él.
        // - Privilegiada (EL1): el frame vive en `stack` (su pila kernel) y
        //   `SPSR=EL1h`. `SP_EL0` no se usa.
        // - Userland (EL0): el frame vive en una pila kernel del pool (EL1-only,
        //   inaccesible a EL0) y `SPSR=EL0t`; `SP_EL0` = tope de `stack` (la pila
        //   de USUARIO, que la placa ubica en una región mapeada EL0).
        let (frame_top, spsr, sp_el0) = if privileged {
            let top = ((stack.as_ptr() as usize) as u64 + stack.len() as u64) & !0xF;
            (top, 0x5u64, 0u64) // EL1h, DAIF=0
        } else {
            let user_sp = ((stack.as_ptr() as usize) as u64 + stack.len() as u64) & !0xF;
            (alloc_kernel_stack_top(), 0x0u64, user_sp) // EL0t, DAIF=0
        };
        let sp = frame_top - CTX_FRAME as u64;
        let f = sp as *mut u64;
        // SAFETY: [sp, sp+336) cae dentro de la pila (kernel) de la tarea.
        unsafe {
            for i in 0..(CTX_FRAME / 8) {
                write_volatile(f.add(i), 0);
            }
            write_volatile(f.add(248 / 8), sp_el0); // SP_EL0
            write_volatile(f.add(256 / 8), entry as *const () as u64); // ELR_EL1
            write_volatile(f.add(264 / 8), spsr); // SPSR_EL1
        }
        Context { sp }
    }

    fn start_first(ctx: *const Self::Context) -> ! {
        // La primera tarea entra con `eret`; su `SPSR` inicial (EL1h, DAIF=0) ya
        // la arranca con IRQs habilitadas, así que el timer la preempta sin
        // necesidad de un `daifclr` aparte. En un despliegue cooperativo sin
        // fuente de IRQ, arrancar desenmascarado es inocuo.
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
