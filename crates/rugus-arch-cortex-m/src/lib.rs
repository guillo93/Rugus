//! Rugus `Arch` backend para ARM Cortex-M.
//!
//! Cubre ARMv7-M (Cortex-M3), ARMv7E-M (Cortex-M4/M7) y ARMv8-M Main
//! (Cortex-M33). Para Cortex-M0/M0+ (ARMv6-M) habrá un backend separado
//! o un cfg de capacidades reducidas (sin MPU configurable, sin algunos
//! intrinsics).
//!
//! Estado G0: stub. Implementación real de context switch + MPU llega
//! en G1/G2.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use rugus_core::arch::{Arch, CriticalGuard};

/// Marker type que implementa [`rugus_core::Arch`] para Cortex-M.
pub struct CortexM;

/// Contexto de tarea (registros calling-convention-saved + SP).
///
/// Layout exacto se decide al implementar el context switch ASM en G1.
#[repr(C)]
#[derive(Default)]
pub struct Context {
    /// Stack pointer de la tarea.
    pub sp: u32,
}

/// Handle de sección crítica: estado previo de PRIMASK.
pub struct SavedPrimask(u32);

impl CriticalGuard for SavedPrimask {}

impl Arch for CortexM {
    type Context = Context;
    type SavedIrq = SavedPrimask;

    const HAS_MEMORY_PROTECTION: bool = true;

    unsafe fn switch_context(_prev: *mut Self::Context, _next: *const Self::Context) {
        // Implementación real (PendSV trigger + ASM) llega en G1.
        // Por ahora, no-op para que compile en G0.
        cortex_m::asm::nop();
    }

    fn enter_critical() -> Self::SavedIrq {
        let primask = cortex_m::register::primask::read();
        cortex_m::interrupt::disable();
        SavedPrimask(primask.is_active() as u32)
    }

    fn exit_critical(saved: Self::SavedIrq) {
        if saved.0 == 0 {
            // SAFETY: estamos restaurando el estado previo de PRIMASK que
            // guardamos al entrar. No habilita IRQs si estaban desactivadas.
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
