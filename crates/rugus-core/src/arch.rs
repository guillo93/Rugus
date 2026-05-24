//! `Arch` trait — contrato que cada crate `rugus-arch-<isa>` implementa.
//!
//! El kernel core nunca llama a instrucciones específicas de CPU; siempre
//! va a través de este trait. Esto permite portar Rugus a una arquitectura
//! nueva escribiendo un crate `rugus-arch-<isa>` que implemente `Arch`,
//! sin tocar `rugus-core`.
//!
//! El trait está intencionalmente acotado: solo las primitivas mínimas
//! comunes a casi cualquier ISA. Features específicas (MPU regions de
//! Cortex-M, MMU page tables de ARMv8-A, PMP de RISC-V) se exponen en el
//! crate arch correspondiente como API propia.

/// Estado opaco de una región crítica que enmascara IRQs.
///
/// `enter_critical` lo crea; `exit_critical` lo consume para restaurar.
/// La intención es que el handle no sea `Copy` para forzar uso correcto.
pub trait CriticalGuard {}

/// Contrato mínimo que cada backend de arquitectura debe cumplir.
///
/// Implementaciones esperadas:
/// - [`rugus_arch_cortex_m::CortexM`](../../rugus_arch_cortex_m/index.html)
///   (ARMv7-M / ARMv7E-M / ARMv8-M).
/// - Futuras: `CortexA`, `RiscV32`, `Avr`.
pub trait Arch: 'static {
    /// Estado de tarea (contexto de registros + SP).
    type Context;

    /// Handle de sección crítica devuelto por `enter_critical`.
    type SavedIrq: CriticalGuard;

    /// `true` si la arch ofrece MPU/MMU/PMP que el kernel puede usar para
    /// aislar dominios de privilegio. Si `false`, los dominios userland
    /// son honor-system.
    const HAS_MEMORY_PROTECTION: bool;

    /// Cambia al contexto destino. Implementación típicamente en ASM
    /// `#[naked]` ubicada en memoria rápida (ITCM en Cortex-M7).
    ///
    /// # Safety
    ///
    /// `prev` y `next` deben apuntar a `Context` válidos. El kernel
    /// scheduler garantiza esto antes de invocar.
    unsafe fn switch_context(prev: *mut Self::Context, next: *const Self::Context);

    /// Enmascara IRQs y devuelve handle para restaurar.
    fn enter_critical() -> Self::SavedIrq;

    /// Restaura la máscara previa de IRQs.
    fn exit_critical(saved: Self::SavedIrq);

    /// Detiene el core hasta la próxima IRQ (para tarea idle).
    fn wait_for_interrupt();

    /// Reset por software.
    fn reset() -> !;
}
