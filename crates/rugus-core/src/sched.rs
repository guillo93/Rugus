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

/// Patrón de relleno de stack para medir el uso máximo (high-water mark).
///
/// En `spawn` el stack se pinta entero con este byte; las posiciones que la
/// tarea nunca tocó lo conservan. La marca de uso máximo es la distancia desde
/// la base hasta el primer byte alterado. `0xA5` es un patrón clásico (no es ni
/// `0x00` ni `0xFF`, valores que el código a veces escribe de forma natural).
pub const STACK_FILL: u8 = 0xA5;

/// Error al registrar una tarea.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// Tabla de tareas llena.
    TooManyTasks,
    /// Stack demasiado pequeño (mínimo 256 bytes).
    StackTooSmall,
    /// Stack userland no apto para la región MPU: la región App-RW de ARMv7-M
    /// exige tamaño potencia de 2 (≥32 B) y base alineada a ese tamaño. Si no se
    /// cumple, la región redondeada cubriría RAM del kernel adyacente → escape
    /// del sandbox. Solo aplica a [`Scheduler::spawn_user`].
    UnalignedUserStack,
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
    /// Dormida hasta que el reloj monotónico alcance este plazo (en ms,
    /// comparado con aritmética envolvente con signo). No elegible por
    /// [`Scheduler::pick_next`] hasta despertar.
    Sleeping(u32),
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
    /// Punto de entrada original, conservado para poder respawnear la tarea tras
    /// un fault: repintar el stack y reconstruir el frame inicial exige re-llamar
    /// a [`Arch::init_task_stack`] con la misma `entry`.
    entry: fn() -> !,
}

/// Número de bandas de prioridad (ver [`Priority`]).
const PRIORITY_BANDS: usize = 3;

/// Scheduler cooperativo con round-robin dentro de cada banda de prioridad.
pub struct Scheduler<A: Arch> {
    tasks: [MaybeUninit<TaskSlot<A>>; MAX_TASKS],
    count: usize,
    current: usize,
    started: bool,
    /// Cursor round-robin por banda: índice de la última tarea servida en cada
    /// [`Priority`]. La rotación de cada banda avanza desde aquí, no desde la
    /// tarea que cede, de modo que las tareas de igual prioridad rotan de forma
    /// justa aunque una banda superior (p. ej. Kernel) se intercale en cada
    /// turno y `from` sea siempre la misma.
    last_served: [usize; PRIORITY_BANDS],
}

impl<A: Arch> Scheduler<A> {
    /// Crea un scheduler vacío.
    pub const fn new() -> Self {
        Self {
            tasks: [const { MaybeUninit::uninit() }; MAX_TASKS],
            count: 0,
            current: 0,
            started: false,
            last_served: [0; PRIORITY_BANDS],
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
        // Invariante del sandbox: una tarea userland obtiene una región MPU
        // dedicada (App-RW) sobre su stack. ARMv7-M exige que esa región sea
        // potencia de 2 y esté alineada a su tamaño. Si el stack no lo cumple,
        // el remapeo redondearía la región y cubriría RAM del kernel vecina,
        // dando acceso de escritura fuera del sandbox. Se rechaza en origen.
        if mode == TaskMode::User {
            let base = stack.as_ptr() as u32;
            let len = stack.len() as u32;
            if len < 32 || !len.is_power_of_two() || base % len != 0 {
                return Err(SpawnError::UnalignedUserStack);
            }
        }
        // Pinta el stack con el patrón antes de montar el frame inicial: el
        // context switch crece desde el tope (direcciones altas) hacia la base,
        // así que las posiciones bajas intactas miden el high-water (ver
        // [`stack_high_water`]). `init_task_stack` reescribe el tope con el
        // frame, lo cual ya cuenta como uso.
        stack.fill(STACK_FILL);
        let stack_len = stack.len() as u32;
        let ctx = A::init_task_stack(stack, entry, mode == TaskMode::Privileged);
        let base = stack.as_ptr() as u32;
        let slot = TaskSlot {
            context: ctx,
            priority,
            state: TaskState::Ready,
            mode,
            domain,
            stack_base: base,
            stack_len,
            entry,
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

    /// Duerme la tarea en ejecución `ms` milisegundos y cede el CPU.
    ///
    /// La tarea no vuelve a ser elegible hasta que el reloj monotónico
    /// ([`Arch::now_ms`]) alcance el plazo. Si hay otra tarea lista se conmuta a
    /// ella; si solo quedan durmientes, el scheduler hace `wfi` hasta el próximo
    /// tick y reevalúa (no hay busy-spin). `ms == 0` equivale a [`Self::yield_now`].
    ///
    /// Cooperativo: el despertar ocurre la próxima vez que una tarea cede o el
    /// `wfi` retorna por interrupción, no de forma preventiva.
    pub fn sleep_ms(&mut self, ms: u32) {
        if !self.started || self.count == 0 {
            return;
        }
        if ms == 0 {
            self.yield_now();
            return;
        }
        let prev_idx = self.current;
        let wake_at = A::now_ms().wrapping_add(ms);
        // SAFETY: prev_idx válido; slot inicializado en spawn.
        unsafe {
            self.tasks[prev_idx].assume_init_mut().state = TaskState::Sleeping(wake_at);
        }
        loop {
            let next_idx = self.pick_next(prev_idx);
            if next_idx != prev_idx {
                self.current = next_idx;
                self.prepare_task_hw(next_idx);
                // SAFETY: índices válidos y contextos inicializados.
                unsafe {
                    let prev =
                        &mut self.tasks[prev_idx].assume_init_mut().context as *mut A::Context;
                    let next = &self.task_ref(next_idx).context as *const A::Context;
                    A::switch_context(prev, next);
                }
                return;
            }
            // Ninguna otra tarea lista. `pick_next` ya despertó las vencidas:
            // si el propio durmiente alcanzó su plazo, sigue sin conmutar.
            if self.task_ref(prev_idx).state == TaskState::Ready {
                return;
            }
            A::wait_for_interrupt();
        }
    }

    /// Mata la tarea faultante y salta a la siguiente; no retorna.
    ///
    /// Invocado desde el fault handler del arch backend. El TCB no registra
    /// logs (se mantiene mínimo y agnóstico del transporte): la observabilidad
    /// del fault es responsabilidad del hook registrado por la plataforma, que
    /// recibe el [`FaultReport`] antes de esta llamada.
    pub fn kill_current_and_resume(&mut self, report: FaultReport) -> ! {
        let _ = report;
        let idx = self.current;
        // SAFETY: idx válido mientras el scheduler está activo.
        unsafe {
            self.tasks[idx].assume_init_mut().state = TaskState::Killed;
        }
        // `idx` ya está muerta, así que `pick_next` nunca la devuelve como
        // lista. Si solo quedan durmientes, espera (wfi) a que alguna venza en
        // vez de abandonar: una tarea dormida sigue viva. Solo si TODAS están
        // muertas se entra en el WFI terminal (la plataforma resetea por
        // watchdog; no hay panic global por diseño).
        let next_idx = loop {
            let n = self.pick_next(idx);
            if n != idx {
                break n;
            }
            if self.all_killed() {
                loop {
                    A::wait_for_interrupt();
                }
            }
            A::wait_for_interrupt();
        };
        self.current = next_idx;
        self.prepare_task_hw(next_idx);
        let next = &self.task_ref(next_idx).context as *const A::Context;
        // SAFETY: índice válido; contexto inicializado en spawn.
        unsafe {
            A::resume_after_fault(next);
        }
    }

    /// Revive una tarea matada por un fault (`Killed` → `Ready`), reconstruyendo
    /// su frame inicial desde cero. Devuelve `true` si la respawneó.
    ///
    /// El supervisor (tarea privilegiada) la invoca para autorreparar una app
    /// caída: repinta el stack con [`STACK_FILL`], reconstruye el contexto con la
    /// `entry` original vía [`Arch::init_task_stack`] y la marca `Ready`. La tarea
    /// arranca limpia (no reanuda donde falló): un fault deja estado indeterminado,
    /// así que un reinicio en frío es la única recuperación segura.
    ///
    /// No-op (devuelve `false`) si `idx` no existe o no está `Killed`: solo se
    /// revive lo que el failsafe mató, nunca se reinicia una tarea viva.
    pub fn respawn(&mut self, idx: usize) -> bool {
        if idx >= self.count || self.task_ref(idx).state != TaskState::Killed {
            return false;
        }
        // SAFETY: idx < count; slot inicializado en spawn. El stack [base,len) es
        // el estático original de la tarea, vivo mientras el scheduler existe; la
        // tarea está Killed (no en ejecución), así que reescribirlo es seguro.
        unsafe {
            let slot = self.tasks[idx].assume_init_mut();
            let base = slot.stack_base as *mut u8;
            let len = slot.stack_len as usize;
            let entry = slot.entry;
            let privileged = slot.mode == TaskMode::Privileged;
            let stack = core::slice::from_raw_parts_mut(base, len);
            stack.fill(STACK_FILL);
            let ctx = A::init_task_stack(stack, entry, privileged);
            slot.context = ctx;
            slot.state = TaskState::Ready;
        }
        true
    }

    /// ID de la tarea en ejecución.
    pub fn current_id(&self) -> TaskId {
        TaskId(self.current as u8)
    }

    /// Dominio lógico de la tarea en ejecución.
    pub fn current_domain(&self) -> Domain {
        self.task_ref(self.current).domain
    }

    /// Región MPU `(base, len)` del stack de la tarea en ejecución si es
    /// userland; `None` si es privilegiada.
    ///
    /// Es la región App-RW exacta sobre la que [`Self::spawn_user`] montó el
    /// sandbox (potencia de 2, alineada). El dispatch de syscalls la usa para
    /// validar punteros de tareas no confiables; para una tarea privilegiada
    /// devuelve `None` porque el kernel se confía a sí mismo (la MPU no aplica
    /// en modo privilegiado).
    pub fn current_user_region(&self) -> Option<(u32, u32)> {
        let slot = self.task_ref(self.current);
        match slot.mode {
            TaskMode::User => Some((slot.stack_base, slot.stack_len)),
            TaskMode::Privileged => None,
        }
    }

    /// Número de tareas registradas.
    pub fn task_count(&self) -> usize {
        self.count
    }

    /// `true` si la tarea `idx` sigue viva (no fue matada por un fault).
    pub fn task_alive(&self, idx: usize) -> bool {
        idx < self.count && self.task_ref(idx).state == TaskState::Ready
    }

    /// Número de tareas que un fault mató (estado `Killed`). Una tarea dormida
    /// sigue contando como viva; solo cuenta las terminadas por el failsafe.
    /// Fuente del indicador de salud del supervisor (LED "degradado").
    pub fn killed_count(&self) -> usize {
        (0..self.count)
            .filter(|&i| self.task_ref(i).state == TaskState::Killed)
            .count()
    }

    /// `true` si la tarea `idx` fue matada por un fault. Una tarea dormida o
    /// lista NO está matada. Índice fuera de rango => `false`.
    pub fn is_killed(&self, idx: usize) -> bool {
        idx < self.count && self.task_ref(idx).state == TaskState::Killed
    }

    /// Tamaño total del stack de la tarea `idx` en bytes (0 si no existe).
    pub fn stack_len(&self, idx: usize) -> u32 {
        if idx >= self.count {
            return 0;
        }
        self.task_ref(idx).stack_len
    }

    /// Uso máximo de stack (high-water mark) de la tarea `idx`, en bytes.
    ///
    /// Cuenta los bytes [`STACK_FILL`] consecutivos desde la base (el extremo
    /// que la tarea alcanza en último lugar) y resta del total: el resultado es
    /// cuánto stack llegó a usar como máximo. Si la marca llega al total, la
    /// tarea pudo haber desbordado y la medida es un límite inferior.
    ///
    /// Coste O(stack_len) — pensado para diagnóstico puntual (`coil`), no para
    /// el camino caliente.
    pub fn stack_high_water(&self, idx: usize) -> u32 {
        if idx >= self.count {
            return 0;
        }
        let slot = self.task_ref(idx);
        let base = slot.stack_base as *const u8;
        let len = slot.stack_len as usize;
        let mut free = 0usize;
        // SAFETY: [base, base+len) es el stack estático de la tarea, vivo
        // mientras el scheduler existe; solo lectura byte a byte.
        unsafe {
            while free < len && *base.add(free) == STACK_FILL {
                free += 1;
            }
        }
        (len - free) as u32
    }

    fn prepare_task_hw(&self, idx: usize) {
        let slot = self.task_ref(idx);
        A::on_task_switch(slot.mode, slot.stack_base, slot.stack_len);
    }

    fn all_killed(&self) -> bool {
        (0..self.count).all(|i| self.task_ref(i).state == TaskState::Killed)
    }

    /// Despierta las tareas dormidas cuyo plazo ya venció.
    ///
    /// Comparación envolvente con signo: `now - wake_at` interpretado como
    /// `i32` es `>= 0` cuando `now` alcanzó o pasó el plazo, correcto a través
    /// del wrap de `u32` mientras un sleep no exceda ~24,8 días (medio rango).
    fn wake_expired(&mut self) {
        let now = A::now_ms();
        for i in 0..self.count {
            if let TaskState::Sleeping(wake_at) = self.task_ref(i).state {
                if now.wrapping_sub(wake_at) as i32 >= 0 {
                    // SAFETY: i < count; slot inicializado en spawn.
                    unsafe {
                        self.tasks[i].assume_init_mut().state = TaskState::Ready;
                    }
                }
            }
        }
    }

    fn pick_next(&mut self, from: usize) -> usize {
        self.wake_expired();
        for band in [Priority::Kernel, Priority::Service, Priority::App] {
            let bi = band as usize;
            let start = self.last_served[bi];
            for offset in 1..=self.count {
                let idx = (start + offset) % self.count;
                if idx == from {
                    continue;
                }
                let slot = self.task_ref(idx);
                if slot.state == TaskState::Ready && slot.priority == band {
                    self.last_served[bi] = idx;
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
