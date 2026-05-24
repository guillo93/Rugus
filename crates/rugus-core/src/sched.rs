//! Scheduler — placeholder pre-G1.
//!
//! El diseño está fijado en `docs/ARCHITECTURE.md` y `docs/ROADMAP.md`:
//! cooperativo round-robin con 3 bandas de prioridad, context switch vía
//! [`Arch::switch_context`](crate::Arch::switch_context).

/// Identificador opaco de tarea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskId(pub u8);

/// Banda de prioridad. Menor número = mayor prioridad.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// Reservado para housekeeping del kernel (watchdog feeder, timer ticks).
    Kernel = 0,
    /// Servicios (red, gráficos). Default para IPC servers.
    Service = 1,
    /// Tareas de aplicación.
    App = 2,
}
