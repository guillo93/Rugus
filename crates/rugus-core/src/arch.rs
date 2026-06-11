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

use crate::sched::TaskMode;

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

    /// Bytes reservados como guarda de pila en la BASE (extremo bajo) de cada
    /// stack. Cuando hay MPU, esta región se programa sin acceso (ni privilegiado
    /// ni userland) para atrapar desbordamientos, y queda activa para la tarea
    /// en ejecución. No es pila utilizable: introspección como el high-water
    /// (`coil`) DEBE saltarla, pues leerla desde la tarea actual dispara un
    /// MemManage. Por defecto `0` (arch sin guarda).
    const STACK_GUARD_BYTES: u32 = 0;

    /// Cambia al contexto destino. Implementación típicamente en ASM
    /// `#[naked]` ubicada en memoria rápida (ITCM en Cortex-M7).
    ///
    /// # Safety
    ///
    /// `prev` y `next` deben apuntar a `Context` válidos. El kernel
    /// scheduler garantiza esto antes de invocar.
    unsafe fn switch_context(prev: *mut Self::Context, next: *const Self::Context);

    /// Construye el contexto inicial sobre `stack` para `entry`.
    fn init_task_stack(stack: &mut [u8], entry: fn() -> !, privileged: bool) -> Self::Context;

    /// Salta a la primera tarea; no retorna.
    fn start_first(ctx: *const Self::Context) -> !;

    /// Restaura una tarea tras matar la faultante; no retorna.
    ///
    /// # Safety
    ///
    /// `ctx` debe apuntar a un contexto válido del scheduler.
    unsafe fn resume_after_fault(ctx: *const Self::Context) -> !;

    /// Hook antes de ejecutar una tarea (MPU / privilegio).
    fn on_task_switch(mode: TaskMode, stack_base: u32, stack_len: u32);

    /// Enmascara IRQs y devuelve handle para restaurar.
    fn enter_critical() -> Self::SavedIrq;

    /// Restaura la máscara previa de IRQs.
    fn exit_critical(saved: Self::SavedIrq);

    /// Detiene el core hasta la próxima IRQ (para tarea idle).
    fn wait_for_interrupt();

    /// Espera ociosa con conocimiento del próximo plazo del scheduler (F5.A).
    ///
    /// El scheduler la invoca cuando no hay ninguna tarea lista y va a dormir el
    /// core. `next_wake_ms` es el resultado de
    /// [`Scheduler::next_wake_ms`](crate::sched::Scheduler::next_wake_ms): los
    /// milisegundos hasta el próximo despertar por reloj, o `None` si sólo una IRQ
    /// externa puede reanudar al sistema.
    ///
    /// El backend puede usarlo para implementar un **tick dinámico**: reprogramar
    /// su temporizador a ese plazo (en vez de interrumpir cada milisegundo) y
    /// re-sincronizar el reloj al despertar, reduciendo los despertares ociosos.
    /// La implementación por defecto ignora el plazo y degrada a
    /// [`Self::wait_for_interrupt`] (tick fijo), de modo que un backend sin tick
    /// dinámico conserva exactamente el comportamiento previo.
    fn idle(next_wake_ms: Option<u32>) {
        let _ = next_wake_ms;
        Self::wait_for_interrupt();
    }

    /// Reloj monotónico en milisegundos para temporización del scheduler.
    ///
    /// Base del sleep/wake cooperativo: el scheduler compara plazos contra este
    /// valor. Envuelve a ~49,7 días (`u32`), por lo que las comparaciones de
    /// plazo usan aritmética envolvente con signo. Si el backend no tiene una
    /// fuente de tiempo inicializada, debe devolver un valor monótono (puede ser
    /// constante 0); en ese caso un `sleep_ms` nunca expira por sí solo.
    fn now_ms() -> u32;

    /// Reset por software.
    fn reset() -> !;
}
