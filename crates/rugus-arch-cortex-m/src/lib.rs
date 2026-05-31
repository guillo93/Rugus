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
pub use mpu::{
    init as mpu_init, layout as mpu_layout, region as mpu_region, remap_app_stack, MpuLayout,
};

use rugus_core::arch::{Arch, CriticalGuard};
use rugus_core::sched::TaskMode;

/// Marker type que implementa [`rugus_core::Arch`] para Cortex-M.
pub struct CortexM;

/// Contexto de tarea: puntero al frame software (r4–r11) en el stack.
#[repr(C)]
#[derive(Default)]
pub struct Context {
    /// Stack pointer de la tarea (PSP tras restore).
    pub sp: u32,
    /// 1 = privilegiada, 0 = userland (nPRIV).
    pub privileged: u32,
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

    fn on_task_switch(mode: TaskMode, stack_base: u32, stack_len: u32) {
        // SAFETY: steal único en cooperativo; MPU escrito en critical section implícita.
        unsafe {
            let mut cp = cortex_m::Peripherals::steal();
            match mode {
                TaskMode::User => {
                    let size = mpu::region_size_for(stack_len as usize);
                    let aligned = mpu::align_down(stack_base, size);
                    mpu::remap_app_stack(&mut cp.MPU, aligned, size);
                }
                TaskMode::Privileged => {
                    mpu::clear_app_stack(&mut cp.MPU);
                }
            }
        }
    }

    fn enter_critical() -> Self::SavedIrq {
        let primask = cortex_m::register::primask::read();
        cortex_m::interrupt::disable();
        SavedPrimask(primask.is_active() as u32)
    }

    fn exit_critical(saved: Self::SavedIrq) {
        if saved.0 == 0 {
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
}

/// Inicializa MPU + fault handlers para la placa dada. Llamar desde `main`
/// antes del scheduler. Cada placa con MPU pasa su [`MpuLayout`].
pub fn platform_init(cp: &mut cortex_m::Peripherals, layout: &MpuLayout) {
    mpu::init(&mut cp.MPU, layout);
    enable_fault_handlers(&mut cp.SCB);
}
