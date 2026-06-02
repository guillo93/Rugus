//! Rugus `Arch` backend para ARM Cortex-M.
//!
//! Cubre ARMv7-M (Cortex-M3), ARMv7E-M (Cortex-M4/M7) y ARMv8-M Main
//! (Cortex-M33). Context switch cooperativo vía PendSV en G1; MPU + SVC en G2.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod exceptions;
mod fault;
mod mpu;
mod svc;
mod switch;
pub mod time;

pub use exceptions::enable_fault_handlers;
pub use fault::set_fault_hook;
pub use mpu::{init as mpu_init, layout as mpu_layout, region as mpu_region, MpuLayout};

use rugus_core::arch::{Arch, CriticalGuard};
use rugus_core::sched::TaskMode;

/// Marker type que implementa [`rugus_core::Arch`] para Cortex-M.
pub struct CortexM;

/// Contexto de tarea: puntero al frame software (r4–r11) en el stack.
///
/// Los campos `mpu_*` se escriben en la región [`mpu::region::APP_STACK`] dentro
/// del propio context switch (PendSV/bootstrap), de forma atómica con la
/// conmutación de registros: garantizan que la región MPU del stack corresponde
/// SIEMPRE a la tarea que se restaura (ver [`mpu::app_region_for`]). El orden de
/// los campos es ABI con el ASM de `switch.rs` (offsets 0/4/8/12).
#[repr(C)]
#[derive(Default)]
pub struct Context {
    /// Stack pointer de la tarea (PSP tras restore).
    pub sp: u32,
    /// 1 = privilegiada, 0 = userland (nPRIV).
    pub privileged: u32,
    /// `RBAR` de la región APP_STACK de esta tarea (sin nº de región ni VALID).
    pub mpu_rbar: u32,
    /// `RASR` de la región APP_STACK: con ENABLE para userland, `0` (región
    /// deshabilitada) para tareas privilegiadas.
    pub mpu_rasr: u32,
    /// `RBAR` de la región STACK_GUARD (32 B sin acceso) en la base del stack.
    pub mpu_guard_rbar: u32,
    /// `RASR` de la región STACK_GUARD: ENABLE + XN + sin acceso. Aplica a TODA
    /// tarea (privilegiada y userland): la región 7 gana el solapamiento.
    pub mpu_guard_rasr: u32,
}

/// Handle de sección crítica: estado previo de PRIMASK.
pub struct SavedPrimask(u32);

impl CriticalGuard for SavedPrimask {}

impl Arch for CortexM {
    type Context = Context;
    type SavedIrq = SavedPrimask;

    const HAS_MEMORY_PROTECTION: bool = true;

    unsafe fn switch_context(prev: *mut Self::Context, next: *const Self::Context) {
        unsafe {
            switch::request_switch(prev, next);
        }
    }

    fn init_task_stack(stack: &mut [u8], entry: fn() -> !, privileged: bool) -> Self::Context {
        switch::init_task_stack(stack, entry, privileged)
    }

    fn start_first(ctx: *const Self::Context) -> ! {
        switch::start_first(ctx)
    }

    unsafe fn resume_after_fault(ctx: *const Self::Context) -> ! {
        // SAFETY: ctx válido; scheduler eligió la siguiente tarea.
        unsafe { switch::resume_after_fault(ctx) }
    }

    fn on_task_switch(_mode: TaskMode, _stack_base: u32, _stack_len: u32) {
        // No-op intencional: la región MPU del stack (APP_STACK) la programa el
        // PROPIO context switch (PendSV/bootstrap) desde los campos `mpu_*` del
        // `Context` de la tarea entrante, de forma atómica con la conmutación de
        // registros.
        //
        // El modelo anterior reprogramaba la MPU AQUÍ, en tiempo de planificación
        // (`prepare_task_hw`), pero el switch real se difiere al PendSV. Bajo
        // preempción por SysTick + cesión cooperativa + reanudación tras fault
        // (todos difieren al PendSV), la MPU podía quedar programada para una
        // tarea distinta de la que el PendSV restauraba → MUNSTKERR al desapilar
        // el frame de una tarea userland con su región de stack deshabilitada.
        // Mover la programación al switch elimina esa clase de desincronizado.
    }

    fn enter_critical() -> Self::SavedIrq {
        // `is_active()` (cortex-m 0.7) es TRUE cuando PRIMASK==0, es decir cuando
        // las interrupciones estaban HABILITADAS al entrar. Guardamos ese estado
        // previo para restaurarlo en `exit_critical` sin desenmascarar de más en
        // secciones críticas anidadas.
        let was_enabled = cortex_m::register::primask::read().is_active();
        cortex_m::interrupt::disable();
        SavedPrimask(was_enabled as u32)
    }

    fn exit_critical(saved: Self::SavedIrq) {
        // Solo reactivamos si las interrupciones estaban habilitadas ANTES de la
        // sección crítica; si ya venían deshabilitadas (anidamiento), las dejamos
        // como estaban. La lógica anterior estaba invertida (reactivaba justo en
        // el caso contrario), lo que dejaba PRIMASK=1 colgado tras una sección
        // crítica normal y, bajo el camino de fault/respawn + preempción, filtraba
        // ese PRIMASK=1 a una tarea userland → el `svc` escalaba a HardFault.
        if saved.0 != 0 {
            // SAFETY: restauramos PRIMASK capturado en enter_critical.
            unsafe { cortex_m::interrupt::enable() }
        }
    }

    fn wait_for_interrupt() {
        cortex_m::asm::wfi();
    }

    fn reset() -> ! {
        cortex_m::peripheral::SCB::sys_reset();
    }

    fn now_ms() -> u32 {
        time::now_ms()
    }
}

/// Inicializa MPU + fault handlers para la placa dada. Llamar desde `main`
/// antes del scheduler. Cada placa con MPU pasa su [`MpuLayout`].
pub fn platform_init(cp: &mut cortex_m::Peripherals, layout: &MpuLayout) {
    init_fpu_context();
    mpu::init(&mut cp.MPU, layout);
    enable_fault_handlers(&mut cp.SCB);
}

/// Configura el modelo de preservación de estado FP para convivir con la MPU.
///
/// `FPCCR.ASPEN=1` (preservación automática: el HW extiende el frame con
/// `S0–S15 + FPSCR` cuando `CONTROL.FPCA=1`) pero `FPCCR.LSPEN=0` (**sin** lazy
/// stacking): el estado FP se apila de forma EAGER en la entrada a excepción.
///
/// El lazy stacking difiere la escritura de `S0–S15` a un puntero (`FPCAR`) que
/// se resuelve al ejecutar la siguiente instrucción FP. Con context switch + MPU
/// por tarea, ese `FPCAR` puede apuntar al stack de una tarea cuya región MPU ya
/// no está activa → el guardado diferido faulta dentro de PendSV (que ejecuta
/// `vstmdb/vldmia`) y entra en cascada. Apilando eager se elimina esa clase de
/// fallo a cambio de algo más de latencia/stack por excepción con estado FP.
///
/// En M3 (`eabi`, sin FPU) no hay registro FPCCR; la función no emite nada.
#[cfg(target_abi = "eabihf")]
fn init_fpu_context() {
    const FPCCR: *mut u32 = 0xE000_EF34 as *mut u32;
    const ASPEN: u32 = 1 << 31;
    const LSPEN: u32 = 1 << 30;
    // SAFETY: registro FPCCR estándar del SCB; init de arranque single-thread.
    unsafe {
        let v = (core::ptr::read_volatile(FPCCR) | ASPEN) & !LSPEN;
        core::ptr::write_volatile(FPCCR, v);
    }
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

#[cfg(not(target_abi = "eabihf"))]
fn init_fpu_context() {}
