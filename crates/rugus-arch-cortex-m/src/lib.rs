//! Rugus `Arch` backend para ARM Cortex-M.
//!
//! Cubre ARMv7-M (Cortex-M3), ARMv7E-M (Cortex-M4/M7) y ARMv8-M Main
//! (Cortex-M33). Context switch cooperativo vía PendSV en G1.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod switch;

use rugus_core::arch::{Arch, CriticalGuard};

/// Marker type que implementa [`rugus_core::Arch`] para Cortex-M.
pub struct CortexM;

/// Contexto de tarea: puntero al frame software (r4–r11) en el stack.
#[repr(C)]
#[derive(Default)]
pub struct Context {
    /// Stack pointer de la tarea (PSP tras restore).
    pub sp: u32,
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

    fn init_task_stack(stack: &mut [u8], entry: fn() -> !) -> Self::Context {
        switch::init_task_stack(stack, entry)
    }

    fn start_first(ctx: *const Self::Context) -> ! {
        switch::start_first(ctx)
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
