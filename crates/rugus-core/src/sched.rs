//! Scheduler cooperativo round-robin — G1 + G2 (userland, fault kill).
//!
//! Máximo [`MAX_TASKS`] tareas en 3 bandas de prioridad ([`Priority`]).
//! El context switch real lo hace [`Arch::switch_context`] (PendSV en Cortex-M).

use crate::domain::Domain;
use crate::fault::FaultReport;
use crate::Arch;
use core::mem::MaybeUninit;

/// Máximo de tareas concurrentes (incluye idle).
pub const MAX_TASKS: usize = 4;

/// Error al registrar una tarea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// Tabla de tareas llena.
    TooManyTasks,
    /// Stack demasiado pequeño (mínimo 256 bytes).
    StackTooSmall,
}

/// Modo de ejecución de la tarea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskMode {
    /// Privilegiada (kernel / drivers).
    Privileged,
    /// Userland con MPU restringida.
    User,
}

/// Estado interno de una tarea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskState {
    Ready,
    Killed,
}

struct TaskSlot<A: Arch> {
    context: A::Context,
    priority: Priority,
    state: TaskState,
    mode: TaskMode,
    domain: Domain,
    /// Base del stack (para remapeo MPU región App).
    stack_base: u32,
    stack_len: u32,
}

/// Scheduler cooperativo con round-robin dentro de cada banda de prioridad.
pub struct Scheduler<A: Arch> {
    tasks: [MaybeUninit<TaskSlot<A>>; MAX_TASKS],
    count: usize,
    current: usize,
    started: bool,
}

impl<A: Arch> Scheduler<A> {
    /// Crea un scheduler vacío.
    pub const fn new() -> Self {
        Self {
            tasks: [const { MaybeUninit::uninit() }; MAX_TASKS],
            count: 0,
            current: 0,
            started: false,
        }
    }

    /// Registra una tarea privilegiada con stack estático y punto de entrada.
    pub fn spawn(
        &mut self,
        stack: &mut [u8],
        entry: fn() -> !,
        priority: Priority,
    ) -> Result<TaskId, SpawnError> {
        self.spawn_inner(stack, entry, priority, TaskMode::Privileged, Domain::Kernel)
    }

    /// Registra una app userland (nPRIV + dominio App).
    pub fn spawn_user(
        &mut self,
        stack: &mut [u8],
        entry: fn() -> !,
        priority: Priority,
    ) -> Result<TaskId, SpawnError> {
        self.spawn_inner(stack, entry, priority, TaskMode::User, Domain::App)
    }

    fn spawn_inner(
        &mut self,
        stack: &mut [u8],
        entry: fn() -> !,
        priority: Priority,
        mode: TaskMode,
        domain: Domain,
    ) -> Result<TaskId, SpawnError> {
        if stack.len() < 256 {
            return Err(SpawnError::StackTooSmall);
        }
        if self.count >= MAX_TASKS {
            return Err(SpawnError::TooManyTasks);
        }
        let ctx = A::init_task_stack(stack, entry, mode == TaskMode::Privileged);
        let base = stack.as_ptr() as u32;
        let slot = TaskSlot {
            context: ctx,
            priority,
            state: TaskState::Ready,
            mode,
            domain,
            stack_base: base,
            stack_len: stack.len() as u32,
        };
        self.tasks[self.count].write(slot);
        let id = TaskId(self.count as u8);
        self.count += 1;
        Ok(id)
    }

    /// Arranca el scheduler; no retorna. La primera tarea elegida depende de
    /// prioridad y round-robin.
    pub fn start(&mut self) -> ! {
        if self.count == 0 {
            A::reset();
        }
        self.started = true;
        self.current = self.pick_next(usize::MAX);
        self.prepare_task_hw(self.current);
        let first = &self.task_ref(self.current).context as *const A::Context;
        A::start_first(first);
    }

    /// Cede el CPU a la siguiente tarea lista (cooperativo).
    pub fn yield_now(&mut self) {
        if !self.started || self.count <= 1 {
            return;
        }
        let prev_idx = self.current;
        let next_idx = self.pick_next(prev_idx);
        if next_idx == prev_idx {
            return;
        }
        self.current = next_idx;
        self.prepare_task_hw(next_idx);
        // SAFETY: índices válidos y contextos inicializados por spawn/start.
        unsafe {
            let prev = &mut self.tasks[prev_idx].assume_init_mut().context as *mut A::Context;
            let next = &self.task_ref(next_idx).context as *const A::Context;
            A::switch_context(prev, next);
        }
    }

    /// Mata la tarea faultante y salta a la siguiente; no retorna.
    ///
    /// Invocado desde el fault handler del arch backend.
    #[allow(unused_variables)]
    pub fn kill_current_and_resume(&mut self, report: FaultReport) -> ! {
        #[cfg(feature = "log")]
        {
            defmt::error!(
                "fault {} domain={} pc={=u32} task={=u8} — killing task",
                report.kind.name(),
                report.domain.name(),
                report.pc,
                report.task_id.0
            );
        }
        let idx = self.current;
        // SAFETY: idx válido mientras el scheduler está activo.
        unsafe {
            self.tasks[idx].assume_init_mut().state = TaskState::Killed;
        }
        let next_idx = self.pick_next(idx);
        if next_idx == idx || self.all_killed() {
            #[cfg(feature = "log")]
            defmt::warn!("all tasks dead after fault; halting");
            loop {
                A::wait_for_interrupt();
            }
        }
        self.current = next_idx;
        self.prepare_task_hw(next_idx);
        let next = &self.task_ref(next_idx).context as *const A::Context;
        // SAFETY: índice válido; contexto inicializado en spawn.
        unsafe {
            A::resume_after_fault(next);
        }
    }

    /// ID de la tarea en ejecución.
    pub fn current_id(&self) -> TaskId {
        TaskId(self.current as u8)
    }

    /// Dominio lógico de la tarea en ejecución.
    pub fn current_domain(&self) -> Domain {
        self.task_ref(self.current).domain
    }

    fn prepare_task_hw(&self, idx: usize) {
        let slot = self.task_ref(idx);
        A::on_task_switch(slot.mode, slot.stack_base, slot.stack_len);
    }

    fn all_killed(&self) -> bool {
        (0..self.count).all(|i| self.task_ref(i).state == TaskState::Killed)
    }

    fn pick_next(&self, from: usize) -> usize {
        for band in [Priority::Kernel, Priority::Service, Priority::App] {
            for offset in 1..=self.count {
                let idx = (from + offset) % self.count;
                if idx == from {
                    continue;
                }
                let slot = self.task_ref(idx);
                if slot.state == TaskState::Ready && slot.priority == band {
                    return idx;
                }
            }
        }
        from
    }

    fn task_ref(&self, idx: usize) -> &TaskSlot<A> {
        // SAFETY: idx < count y solo se escribe en spawn.
        unsafe { self.tasks[idx].assume_init_ref() }
    }
}

impl<A: Arch> Default for Scheduler<A> {
    fn default() -> Self {
        Self::new()
    }
}

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
