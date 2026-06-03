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

/// Máximo de mutexes gestionados por el kernel (con herencia de prioridad).
pub const MAX_MUTEXES: usize = 4;

/// Máximo de semáforos contadores gestionados por el kernel.
pub const MAX_SEMAPHORES: usize = 4;

/// Máximo de canales IPC bloqueantes gestionados por el kernel.
pub const MAX_CHANNELS: usize = 4;

/// Capacidad (mensajes en vuelo) del buffer de cada canal IPC.
pub const CHAN_CAPACITY: usize = 4;

/// Valor de `timeout_ms` que indica "esperar indefinidamente" (sin plazo).
pub const TIMEOUT_FOREVER: u32 = u32::MAX;

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
    /// Bloqueada esperando el mutex indicado (índice). No elegible hasta que el
    /// dueño lo libere y le transfiera la propiedad ([`Scheduler::mutex_unlock`]).
    BlockedMutex(u8),
    /// Bloqueada esperando el semáforo indicado. La despierta un
    /// [`Scheduler::sem_post`].
    BlockedSem(u8),
    /// Bloqueada esperando recibir de un canal IPC (índice). La despierta un
    /// [`Scheduler::chan_send`] o el vencimiento de su plazo (`block_deadline`).
    BlockedRecv(u8),
    /// Bloqueada esperando que un canal IPC lleno tenga hueco (índice). La
    /// despierta un [`Scheduler::chan_recv`] o el vencimiento de su plazo.
    BlockedSend(u8),
    Killed,
}

/// Bloque de control de un mutex con herencia de prioridad.
///
/// El dueño hereda la prioridad efectiva más alta entre sus waiters mientras lo
/// retiene, de modo que una tarea de baja prioridad que bloquea a una de alta
/// no puede ser interrumpida indefinidamente por una de prioridad media
/// (inversión de prioridad acotada). La herencia se recalcula en cada lock/unlock.
#[derive(Clone, Copy)]
struct MutexCb {
    /// Índice de la tarea dueña, o `None` si está libre.
    owner: Option<u8>,
    /// Bitmask de tareas bloqueadas esperando este mutex.
    waiters: u8,
}

impl MutexCb {
    const fn new() -> Self {
        Self {
            owner: None,
            waiters: 0,
        }
    }
}

/// Bloque de control de un semáforo contador.
#[derive(Clone, Copy)]
struct SemCb {
    /// Permisos disponibles. `sem_wait` consume uno (o bloquea); `sem_post` lo
    /// devuelve (o despierta a un waiter).
    count: u32,
    /// Bitmask de tareas bloqueadas esperando un permiso.
    waiters: u8,
}

impl SemCb {
    const fn new() -> Self {
        Self {
            count: 0,
            waiters: 0,
        }
    }
}

/// Bloque de control de un canal IPC bloqueante con buffer FIFO acotado.
///
/// Un `send` encola un mensaje (o bloquea con plazo si el buffer está lleno) y
/// despierta al receptor bloqueado de mayor prioridad. Un `recv` desencola (o
/// bloquea con plazo si está vacío) y despierta al emisor bloqueado de mayor
/// prioridad al liberar hueco. La latencia de bloqueo está acotada por el plazo
/// (`timeout_ms`), sin busy-wait: el durmiente cede el CPU y el scheduler lo
/// reevalúa al despertar.
#[derive(Clone, Copy)]
struct ChanCb {
    /// Buffer circular de mensajes opacos (`u32`).
    buf: [u32; CHAN_CAPACITY],
    /// Índice del primer mensaje válido.
    head: u8,
    /// Número de mensajes en vuelo (`0..=CHAN_CAPACITY`).
    len: u8,
    /// Bitmask de tareas bloqueadas esperando recibir.
    recv_waiters: u8,
    /// Bitmask de tareas bloqueadas esperando hueco para enviar.
    send_waiters: u8,
}

impl ChanCb {
    const fn new() -> Self {
        Self {
            buf: [0; CHAN_CAPACITY],
            head: 0,
            len: 0,
            recv_waiters: 0,
            send_waiters: 0,
        }
    }

    /// Encola `msg` si hay hueco. `true` si lo encoló.
    fn push(&mut self, msg: u32) -> bool {
        if (self.len as usize) >= CHAN_CAPACITY {
            return false;
        }
        let tail = (self.head as usize + self.len as usize) % CHAN_CAPACITY;
        self.buf[tail] = msg;
        self.len += 1;
        true
    }

    /// Desencola el mensaje más antiguo (FIFO) si lo hay.
    fn pop(&mut self) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        let msg = self.buf[self.head as usize];
        self.head = ((self.head as usize + 1) % CHAN_CAPACITY) as u8;
        self.len -= 1;
        Some(msg)
    }
}

struct TaskSlot<A: Arch> {
    context: A::Context,
    /// Prioridad EFECTIVA usada por [`Scheduler::pick_next`]. Puede subir por
    /// encima de [`Self::base_priority`] mientras la tarea retiene un mutex con
    /// waiters de mayor prioridad (herencia de prioridad).
    priority: Priority,
    /// Prioridad BASE con la que la tarea fue creada. Es el suelo al que vuelve
    /// la prioridad efectiva al soltar todos los mutexes heredados.
    base_priority: Priority,
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
    /// Plazo (ms, reloj monotónico) tras el cual un bloqueo IPC vence por
    /// timeout. `None` mientras la tarea no está bloqueada con plazo o el bloqueo
    /// es indefinido ([`TIMEOUT_FOREVER`]).
    block_deadline: Option<u32>,
    /// Periodo de liveness (ms): la tarea debe hacer `checkin` antes de que pase
    /// este intervalo o el monitor la considera colgada. `None` = no monitorizada.
    liveness_period: Option<u32>,
    /// Plazo absoluto (ms) del próximo checkin de liveness. Se renueva en cada
    /// [`Scheduler::liveness_checkin`]; el monitor declara colgada a la tarea si
    /// el reloj lo rebasa. `None` si no está monitorizada.
    liveness_deadline: Option<u32>,
}

/// Número de bandas de prioridad (ver [`Priority`]).
const PRIORITY_BANDS: usize = 3;

/// Quantum de planificación en ticks de SysTick (1 ms/tick → 10 ms de rodaja).
///
/// Cada [`Scheduler::preempt_tick`] (un tick) acumula; al alcanzar este número
/// el scheduler fuerza un cambio de contexto round-robin dentro de la banda de
/// mayor prioridad lista. Es lo que impide que una tarea CPU-bound que nunca
/// cede monopolice el núcleo: la preempción la expulsa al vencer su rodaja.
const SLICE_TICKS: u32 = 10;

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
    /// Ticks de SysTick acumulados en la rodaja actual. [`Self::preempt_tick`]
    /// lo incrementa; al llegar a `SLICE_TICKS` fuerza un cambio de contexto
    /// preemptivo y lo reinicia.
    slice_ticks: u32,
    /// Bloques de control de mutexes con herencia de prioridad (id = índice).
    mutexes: [MutexCb; MAX_MUTEXES],
    /// Bloques de control de semáforos contadores (id = índice).
    semaphores: [SemCb; MAX_SEMAPHORES],
    /// Bloques de control de canales IPC bloqueantes (id = índice).
    channels: [ChanCb; MAX_CHANNELS],
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
            slice_ticks: 0,
            mutexes: [MutexCb::new(); MAX_MUTEXES],
            semaphores: [SemCb::new(); MAX_SEMAPHORES],
            channels: [ChanCb::new(); MAX_CHANNELS],
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
            base_priority: priority,
            state: TaskState::Ready,
            mode,
            domain,
            stack_base: base,
            stack_len,
            entry,
            block_deadline: None,
            liveness_period: None,
            liveness_deadline: None,
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

    /// Cede el CPU a la siguiente tarea lista.
    ///
    /// Toda la elección + mutación de estado del scheduler ocurre con IRQs
    /// enmascaradas: así la preempción por SysTick ([`Self::preempt_tick`]) no
    /// puede entrar a medias y aliasar/corromper el estado. El `switch_context`
    /// solo *pende* el PendSV; con las IRQs aún enmascaradas el cambio queda
    /// diferido hasta el `exit_critical`, que al desenmascarar lo dispara — y,
    /// ante empate con un SysTick pendiente, PendSV (núm. de excepción menor)
    /// gana, de modo que SysTick nunca observa el `current` ya actualizado pero
    /// sin haber conmutado todavía.
    pub fn yield_now(&mut self) {
        if !self.started || self.count <= 1 {
            return;
        }
        let guard = A::enter_critical();
        let prev_idx = self.current;
        let next_idx = self.pick_next(prev_idx);
        if next_idx != prev_idx {
            self.current = next_idx;
            self.slice_ticks = 0;
            self.prepare_task_hw(next_idx);
            // SAFETY: índices válidos y contextos inicializados por spawn/start.
            unsafe {
                let prev = &mut self.tasks[prev_idx].assume_init_mut().context as *mut A::Context;
                let next = &self.task_ref(next_idx).context as *const A::Context;
                A::switch_context(prev, next);
            }
        }
        A::exit_critical(guard);
    }

    /// Preempción por tick de SysTick: invocada desde la ISR de SysTick (1 ms).
    ///
    /// Acumula ticks; al vencer la rodaja (`SLICE_TICKS`) elige round-robin la
    /// siguiente tarea de la banda de mayor prioridad lista y pende un cambio de
    /// contexto. El PendSV tiene la misma prioridad que SysTick y un núm. de
    /// excepción menor, así que hace *tail-chain* al salir de esta ISR.
    ///
    /// Exclusión mutua con el camino cooperativo: este método solo corre en la
    /// ISR de SysTick, que el código en modo hilo enmascara mientras toca el
    /// scheduler ([`Self::yield_now`]/[`Self::sleep_ms`]). No reentra.
    pub fn preempt_tick(&mut self) {
        if !self.started || self.count <= 1 {
            return;
        }
        self.slice_ticks += 1;
        if self.slice_ticks < SLICE_TICKS {
            return;
        }
        self.slice_ticks = 0;
        let prev_idx = self.current;
        let next_idx = self.pick_next(prev_idx);
        if next_idx == prev_idx {
            return;
        }
        self.current = next_idx;
        self.prepare_task_hw(next_idx);
        // SAFETY: índices válidos y contextos inicializados; acceso exclusivo
        // (el modo hilo enmascara SysTick mientras toca el scheduler).
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
        {
            let guard = A::enter_critical();
            // SAFETY: prev_idx válido; slot inicializado en spawn.
            unsafe {
                self.tasks[prev_idx].assume_init_mut().state = TaskState::Sleeping(wake_at);
            }
            A::exit_critical(guard);
        }
        loop {
            // Cada iteración evalúa el scheduler con IRQs enmascaradas (excluye a
            // [`Self::preempt_tick`]); el `wfi` espera fuera de la máscara para
            // que SysTick avance el reloj y despierte a los durmientes vencidos.
            let guard = A::enter_critical();
            let next_idx = self.pick_next(prev_idx);
            if next_idx != prev_idx {
                self.current = next_idx;
                self.slice_ticks = 0;
                self.prepare_task_hw(next_idx);
                // SAFETY: índices válidos y contextos inicializados.
                unsafe {
                    let prev =
                        &mut self.tasks[prev_idx].assume_init_mut().context as *mut A::Context;
                    let next = &self.task_ref(next_idx).context as *const A::Context;
                    A::switch_context(prev, next);
                }
                // El switch queda diferido al desenmascarar (PendSV gana el empate
                // con SysTick): al volver a esta tarea, retornamos.
                A::exit_critical(guard);
                return;
            }
            // Ninguna otra tarea lista. `pick_next` ya despertó las vencidas:
            // si el propio durmiente alcanzó su plazo, sigue sin conmutar.
            if self.task_ref(prev_idx).state == TaskState::Ready {
                A::exit_critical(guard);
                return;
            }
            A::exit_critical(guard);
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
        // Suelta cualquier mutex/semáforo que la tarea muerta retuviera o
        // esperase: si no, sus waiters quedarían bloqueados para siempre y la
        // propiedad de un mutex se filtraría (deadlock estructural).
        self.release_task_sync(idx);
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
        // Reinicia la rodaja igual que TODA conmutación (yield_now/preempt_tick):
        // la tarea recién reanudada por el failsafe aún no ha ejecutado. Sin esto,
        // un SysTick que quedó pendiente durante el manejo del fault (p. ej. el
        // log de `fault_hook`) haría tail-chain tras el PendSV de resume y, si la
        // rodaja ya estaba vencida, `preempt_tick` conmutaría de inmediato
        // GUARDANDO el PSP rancio de la tarea reanudada (que no llegó a correr) →
        // contexto corrupto → MUNSTKERR al desapilar más tarde. Mantener el
        // invariante "cada switch reinicia la rodaja" cierra esa carrera.
        self.slice_ticks = 0;
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
            // Arranca limpia: sin prioridad heredada de su vida anterior.
            slot.priority = slot.base_priority;
            // El monitor de liveness se rearma cuando la tarea revivida vuelva a
            // llamar a `liveness_checkin`/`set_liveness_period`; si conservara el
            // plazo viejo, el supervisor la declararía colgada de inmediato.
            slot.liveness_period = None;
            slot.liveness_deadline = None;
        }
        true
    }

    /// Libera todos los objetos de sincronización ligados a la tarea `idx`:
    /// la quita de las listas de waiters, y cada mutex que poseía pasa a su
    /// siguiente waiter (o queda libre). Idempotente; usado al matar/respawnear.
    fn release_task_sync(&mut self, idx: usize) {
        let bit = 1u8 << idx;
        for id in 0..MAX_MUTEXES {
            self.mutexes[id].waiters &= !bit;
            if self.mutexes[id].owner == Some(idx as u8) {
                match self.highest_priority_waiter(self.mutexes[id].waiters) {
                    Some(w) => {
                        self.mutexes[id].waiters &= !(1 << w);
                        self.mutexes[id].owner = Some(w as u8);
                        // SAFETY: w < count; slot inicializado en spawn.
                        unsafe {
                            self.tasks[w].assume_init_mut().state = TaskState::Ready;
                        }
                        self.recompute_priority(w);
                    }
                    None => self.mutexes[id].owner = None,
                }
            }
        }
        for id in 0..MAX_SEMAPHORES {
            self.semaphores[id].waiters &= !bit;
        }
        for id in 0..MAX_CHANNELS {
            self.channels[id].recv_waiters &= !bit;
            self.channels[id].send_waiters &= !bit;
        }
    }

    // --- Monitor de liveness / deadline por tarea (F4.3) ---

    /// Arma la monitorización de liveness de la tarea `idx`: a partir de ahora
    /// debe renovar su plazo (vía [`Self::liveness_checkin`]) cada `period_ms`
    /// como máximo, o el monitor la considerará colgada. Fija el primer plazo en
    /// `ahora + period_ms`. No-op si `idx` no existe o `period_ms` es 0.
    ///
    /// Detecta el fallo que el watchdog y el fault containment NO ven: una tarea
    /// que sigue "viva" (no crashea) pero deja de progresar (bucle infinito,
    /// deadlock de lógica, espera que nunca llega).
    ///
    /// `period_ms == 0` **desarma** la monitorización de la tarea. No-op si `idx`
    /// no existe.
    pub fn set_liveness_period(&mut self, idx: usize, period_ms: u32) {
        if idx >= self.count {
            return;
        }
        // SAFETY: idx < count; slot inicializado en spawn.
        unsafe {
            let slot = self.tasks[idx].assume_init_mut();
            if period_ms == 0 {
                slot.liveness_period = None;
                slot.liveness_deadline = None;
            } else {
                slot.liveness_period = Some(period_ms);
                slot.liveness_deadline = Some(A::now_ms().wrapping_add(period_ms));
            }
        }
    }

    /// Renueva el plazo de liveness de la tarea `idx` a `ahora + periodo`. Es el
    /// "latido" que la tarea emite para demostrar que progresa. No-op si la
    /// tarea no existe o no tiene la monitorización armada.
    pub fn liveness_checkin(&mut self, idx: usize) {
        if idx >= self.count {
            return;
        }
        // SAFETY: idx < count; slot inicializado en spawn.
        unsafe {
            let slot = self.tasks[idx].assume_init_mut();
            if let Some(period) = slot.liveness_period {
                slot.liveness_deadline = Some(A::now_ms().wrapping_add(period));
            }
        }
    }

    /// Latido de liveness de la tarea en ejecución (azúcar para el syscall
    /// `Checkin`): renueva el plazo de la tarea actual.
    pub fn liveness_checkin_current(&mut self) {
        let idx = self.current;
        self.liveness_checkin(idx);
    }

    /// Escanea las tareas monitorizadas y devuelve el índice de la primera cuyo
    /// plazo de liveness ha vencido (sigue viva pero dejó de hacer checkin).
    /// `None` si ninguna está colgada. El supervisor lo consulta para recuperar
    /// (force_kill + respawn) tareas que el fault containment no captura.
    ///
    /// Excluye a la tarea en ejecución (que por definición está progresando) y a
    /// las `Killed` (ya las gestiona el respawn por fault).
    pub fn liveness_overdue(&self) -> Option<usize> {
        let now = A::now_ms();
        for idx in 0..self.count {
            if idx == self.current {
                continue;
            }
            let slot = self.task_ref(idx);
            if slot.state == TaskState::Killed {
                continue;
            }
            if let Some(deadline) = slot.liveness_deadline {
                // Comparación monotónica resistente a wrap (igual que sleep).
                if now.wrapping_sub(deadline) as i32 >= 0 {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Mata por la fuerza una tarea viva (no la actual) para recuperarla: la
    /// marca `Killed`, libera sus objetos de sincronización y desarma su
    /// liveness. El supervisor la combina con [`Self::respawn`] para reiniciar en
    /// frío una tarea colgada. Devuelve `true` si la mató.
    ///
    /// No-op (devuelve `false`) si `idx` no existe, es la tarea en ejecución
    /// (no se autodestruye desde aquí: ese camino es `kill_current_and_resume`)
    /// o ya estaba `Killed`.
    pub fn force_kill(&mut self, idx: usize) -> bool {
        if idx >= self.count || idx == self.current {
            return false;
        }
        if self.task_ref(idx).state == TaskState::Killed {
            return false;
        }
        // SAFETY: idx < count; slot inicializado en spawn.
        unsafe {
            let slot = self.tasks[idx].assume_init_mut();
            slot.state = TaskState::Killed;
            slot.liveness_period = None;
            slot.liveness_deadline = None;
        }
        self.release_task_sync(idx);
        true
    }

    // --- Sincronización con herencia de prioridad (F4.1) ---

    /// Intenta tomar el mutex `id` sin bloquear. `true` si lo adquirió (o ya era
    /// suyo); `false` si lo retiene otra tarea. No duerme ni conmuta: apto para
    /// uso desde contextos donde no se puede ceder (p. ej. selftest de arranque
    /// antes de [`Self::start`]).
    pub fn mutex_try_lock(&mut self, id: usize) -> bool {
        if id >= MAX_MUTEXES {
            return false;
        }
        let cur = self.current as u8;
        match self.mutexes[id].owner {
            None => {
                self.mutexes[id].owner = Some(cur);
                true
            }
            Some(o) => o == cur,
        }
    }

    /// Toma el mutex `id`; si lo retiene otra tarea, bloquea la actual y le
    /// **presta su prioridad al dueño** (priority inheritance) hasta que lo
    /// libere. Devuelve 0, o [`Errno::Einval`](crate::Errno) si `id` no existe.
    ///
    /// Limitación conocida: la herencia es de un nivel (no transitiva en cadenas
    /// dueño→dueño); suficiente con [`MAX_TASKS`]=4 y validado por tests host.
    pub fn mutex_lock(&mut self, id: usize) -> i32 {
        if id >= MAX_MUTEXES {
            return crate::Errno::Einval as i32;
        }
        if !self.started {
            // Sin scheduler activo no se puede bloquear: degradar a try-lock.
            self.mutex_try_lock(id);
            return 0;
        }
        let me = self.current;
        let guard = A::enter_critical();
        let acquired = self.mutex_acquire(id, me);
        A::exit_critical(guard);
        if !acquired {
            self.switch_until_ready(me);
        }
        0
    }

    /// Libera el mutex `id` (debe ser el dueño), transfiere la propiedad al
    /// waiter de mayor prioridad si lo hay y suelta la prioridad heredada.
    /// Devuelve 0, [`Errno::Einval`](crate::Errno) si `id` no existe o
    /// [`Errno::Edenied`](crate::Errno) si el llamante no es el dueño.
    pub fn mutex_unlock(&mut self, id: usize) -> i32 {
        if id >= MAX_MUTEXES {
            return crate::Errno::Einval as i32;
        }
        let me = self.current;
        let guard = A::enter_critical();
        if self.mutexes[id].owner != Some(me as u8) {
            A::exit_critical(guard);
            return crate::Errno::Edenied as i32;
        }
        match self.highest_priority_waiter(self.mutexes[id].waiters) {
            Some(w) => {
                self.mutexes[id].waiters &= !(1 << w);
                self.mutexes[id].owner = Some(w as u8);
                // SAFETY: w < count; slot inicializado en spawn.
                unsafe {
                    self.tasks[w].assume_init_mut().state = TaskState::Ready;
                }
            }
            None => self.mutexes[id].owner = None,
        }
        // Suelta la prioridad prestada: recomputa el efectivo desde la base y los
        // mutexes que aún retiene.
        self.recompute_priority(me);
        A::exit_critical(guard);
        // Si despertamos a alguien (potencialmente de mayor prioridad), cede para
        // que el scheduler lo respete de inmediato.
        if self.started {
            self.yield_now();
        }
        0
    }

    /// Inicializa el semáforo `id` con `count` permisos. Llamar desde `main`
    /// antes de arrancar tareas. No-op si `id` no existe.
    pub fn sem_init(&mut self, id: usize, count: u32) {
        if id < MAX_SEMAPHORES {
            self.semaphores[id].count = count;
        }
    }

    /// Intenta consumir un permiso del semáforo `id` sin bloquear. `true` si lo
    /// consumió.
    pub fn sem_try_wait(&mut self, id: usize) -> bool {
        if id >= MAX_SEMAPHORES {
            return false;
        }
        if self.semaphores[id].count > 0 {
            self.semaphores[id].count -= 1;
            true
        } else {
            false
        }
    }

    /// Consume un permiso del semáforo `id`; si no hay, bloquea la tarea actual
    /// hasta que un [`Self::sem_post`] la despierte. Devuelve 0 o
    /// [`Errno::Einval`](crate::Errno) si `id` no existe.
    pub fn sem_wait(&mut self, id: usize) -> i32 {
        if id >= MAX_SEMAPHORES {
            return crate::Errno::Einval as i32;
        }
        if !self.started {
            self.sem_try_wait(id);
            return 0;
        }
        let me = self.current;
        let guard = A::enter_critical();
        let got = if self.semaphores[id].count > 0 {
            self.semaphores[id].count -= 1;
            true
        } else {
            self.semaphores[id].waiters |= 1 << me;
            // SAFETY: me < count; slot inicializado en spawn.
            unsafe {
                self.tasks[me].assume_init_mut().state = TaskState::BlockedSem(id as u8);
            }
            false
        };
        A::exit_critical(guard);
        if !got {
            self.switch_until_ready(me);
        }
        0
    }

    /// Devuelve un permiso al semáforo `id`: despierta al waiter de mayor
    /// prioridad si lo hay, o incrementa el contador. Devuelve 0 o
    /// [`Errno::Einval`](crate::Errno) si `id` no existe.
    pub fn sem_post(&mut self, id: usize) -> i32 {
        if id >= MAX_SEMAPHORES {
            return crate::Errno::Einval as i32;
        }
        let guard = A::enter_critical();
        match self.highest_priority_waiter(self.semaphores[id].waiters) {
            Some(w) => {
                self.semaphores[id].waiters &= !(1 << w);
                // SAFETY: w < count; slot inicializado en spawn.
                unsafe {
                    self.tasks[w].assume_init_mut().state = TaskState::Ready;
                }
            }
            None => self.semaphores[id].count = self.semaphores[id].count.saturating_add(1),
        }
        A::exit_critical(guard);
        if self.started {
            self.yield_now();
        }
        0
    }

    /// Prioridad efectiva (posiblemente heredada) de la tarea `idx`, como número
    /// (menor = mayor prioridad). Diagnóstico; `0xFF` si `idx` no existe.
    pub fn task_priority(&self, idx: usize) -> u8 {
        if idx >= self.count {
            return 0xFF;
        }
        self.task_ref(idx).priority as u8
    }

    // --- IPC bloqueante con timeout/deadline (F4.2) ---

    /// Calcula el plazo absoluto (ms) de un bloqueo con `timeout_ms` relativo.
    /// `None` si es no bloqueante (`0`) o indefinido ([`TIMEOUT_FOREVER`]).
    fn block_deadline(timeout_ms: u32) -> Option<u32> {
        if timeout_ms == 0 || timeout_ms == TIMEOUT_FOREVER {
            None
        } else {
            Some(A::now_ms().wrapping_add(timeout_ms))
        }
    }

    /// Despierta al receptor bloqueado de mayor prioridad del canal `chan`, si lo
    /// hay (lo saca de la lista de waiters y lo marca listo). El llamante debe
    /// sostener la sección crítica. Devuelve `true` si despertó a alguien.
    fn wake_one_recv(&mut self, chan: usize) -> bool {
        match self.highest_priority_waiter(self.channels[chan].recv_waiters) {
            Some(w) => {
                self.channels[chan].recv_waiters &= !(1 << w);
                // SAFETY: w < count; slot inicializado en spawn.
                unsafe {
                    let slot = self.tasks[w].assume_init_mut();
                    slot.state = TaskState::Ready;
                    slot.block_deadline = None;
                }
                true
            }
            None => false,
        }
    }

    /// Despierta al emisor bloqueado de mayor prioridad del canal `chan`, si lo
    /// hay. El llamante debe sostener la sección crítica. `true` si despertó.
    fn wake_one_send(&mut self, chan: usize) -> bool {
        match self.highest_priority_waiter(self.channels[chan].send_waiters) {
            Some(w) => {
                self.channels[chan].send_waiters &= !(1 << w);
                // SAFETY: w < count; slot inicializado en spawn.
                unsafe {
                    let slot = self.tasks[w].assume_init_mut();
                    slot.state = TaskState::Ready;
                    slot.block_deadline = None;
                }
                true
            }
            None => false,
        }
    }

    /// Envía `msg` por el canal IPC `chan`, bloqueando hasta `timeout_ms` ms si el
    /// buffer está lleno (`0` = no bloqueante; [`TIMEOUT_FOREVER`] = indefinido).
    ///
    /// Devuelve `0` al encolar, [`Errno::Ebusy`](crate::Errno) si está lleno y no
    /// bloquea, [`Errno::Etimedout`](crate::Errno) si vence el plazo, o
    /// [`Errno::Einval`](crate::Errno) si `chan` no existe. Latencia acotada por
    /// el plazo, sin busy-wait: el emisor cede el CPU mientras espera hueco.
    pub fn chan_send(&mut self, chan: usize, msg: u32, timeout_ms: u32) -> i32 {
        if chan >= MAX_CHANNELS {
            return crate::Errno::Einval as i32;
        }
        // Antes de arrancar el scheduler no se puede ceder: degrada a no bloqueante.
        if !self.started {
            return if self.channels[chan].push(msg) {
                0
            } else {
                crate::Errno::Ebusy as i32
            };
        }
        let me = self.current;
        let deadline = Self::block_deadline(timeout_ms);
        loop {
            let guard = A::enter_critical();
            if self.channels[chan].push(msg) {
                let woke = self.wake_one_recv(chan);
                A::exit_critical(guard);
                if woke {
                    self.yield_now();
                }
                return 0;
            }
            if timeout_ms == 0 {
                A::exit_critical(guard);
                return crate::Errno::Ebusy as i32;
            }
            if let Some(d) = deadline {
                if A::now_ms().wrapping_sub(d) as i32 >= 0 {
                    A::exit_critical(guard);
                    return crate::Errno::Etimedout as i32;
                }
            }
            self.channels[chan].send_waiters |= 1 << me;
            // SAFETY: me < count; slot inicializado en spawn.
            unsafe {
                let slot = self.tasks[me].assume_init_mut();
                slot.state = TaskState::BlockedSend(chan as u8);
                slot.block_deadline = deadline;
            }
            A::exit_critical(guard);
            self.switch_until_ready(me);
        }
    }

    /// Recibe un mensaje del canal IPC `chan` en `out`, bloqueando hasta
    /// `timeout_ms` ms si está vacío (`0` = no bloqueante; [`TIMEOUT_FOREVER`] =
    /// indefinido).
    ///
    /// Devuelve `0` y escribe `out` al recibir, [`Errno::Ebusy`](crate::Errno) si
    /// está vacío y no bloquea, [`Errno::Etimedout`](crate::Errno) si vence el
    /// plazo, o [`Errno::Einval`](crate::Errno) si `chan` no existe.
    pub fn chan_recv(&mut self, chan: usize, timeout_ms: u32, out: &mut u32) -> i32 {
        if chan >= MAX_CHANNELS {
            return crate::Errno::Einval as i32;
        }
        if !self.started {
            return match self.channels[chan].pop() {
                Some(m) => {
                    *out = m;
                    0
                }
                None => crate::Errno::Ebusy as i32,
            };
        }
        let me = self.current;
        let deadline = Self::block_deadline(timeout_ms);
        loop {
            let guard = A::enter_critical();
            if let Some(m) = self.channels[chan].pop() {
                *out = m;
                let woke = self.wake_one_send(chan);
                A::exit_critical(guard);
                if woke {
                    self.yield_now();
                }
                return 0;
            }
            if timeout_ms == 0 {
                A::exit_critical(guard);
                return crate::Errno::Ebusy as i32;
            }
            if let Some(d) = deadline {
                if A::now_ms().wrapping_sub(d) as i32 >= 0 {
                    A::exit_critical(guard);
                    return crate::Errno::Etimedout as i32;
                }
            }
            self.channels[chan].recv_waiters |= 1 << me;
            // SAFETY: me < count; slot inicializado en spawn.
            unsafe {
                let slot = self.tasks[me].assume_init_mut();
                slot.state = TaskState::BlockedRecv(chan as u8);
                slot.block_deadline = deadline;
            }
            A::exit_critical(guard);
            self.switch_until_ready(me);
        }
    }

    /// Adquisición de mutex (solo contabilidad, sin conmutar). Devuelve `true` si
    /// la tomó; `false` si bloqueó la tarea `me` y prestó prioridad al dueño. El
    /// llamante debe sostener la sección crítica.
    fn mutex_acquire(&mut self, id: usize, me: usize) -> bool {
        match self.mutexes[id].owner {
            None => {
                self.mutexes[id].owner = Some(me as u8);
                true
            }
            Some(o) if o as usize == me => true,
            Some(owner) => {
                self.mutexes[id].waiters |= 1 << me;
                // SAFETY: me < count; slot inicializado en spawn.
                unsafe {
                    self.tasks[me].assume_init_mut().state = TaskState::BlockedMutex(id as u8);
                }
                self.recompute_priority(owner as usize);
                false
            }
        }
    }

    /// Elige el índice del waiter de mayor prioridad efectiva en `mask` (empate →
    /// menor índice), o `None` si la máscara está vacía.
    fn highest_priority_waiter(&self, mask: u8) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut w = mask;
        while w != 0 {
            let i = w.trailing_zeros() as usize;
            w &= w - 1;
            match best {
                None => best = Some(i),
                Some(b)
                    if (self.task_ref(i).priority as u8) < (self.task_ref(b).priority as u8) =>
                {
                    best = Some(i)
                }
                _ => {}
            }
        }
        best
    }

    /// Recalcula la prioridad efectiva de la tarea `t`: su base, elevada a la
    /// prioridad efectiva más alta entre los waiters de TODOS los mutexes que
    /// retiene. Núcleo de la herencia de prioridad.
    fn recompute_priority(&mut self, t: usize) {
        let mut eff = self.task_ref(t).base_priority as u8;
        for id in 0..MAX_MUTEXES {
            if self.mutexes[id].owner == Some(t as u8) {
                let mut w = self.mutexes[id].waiters;
                while w != 0 {
                    let i = w.trailing_zeros() as usize;
                    w &= w - 1;
                    let wp = self.task_ref(i).priority as u8;
                    if wp < eff {
                        eff = wp;
                    }
                }
            }
        }
        let p = match eff {
            0 => Priority::Kernel,
            1 => Priority::Service,
            _ => Priority::App,
        };
        // SAFETY: t < count; slot inicializado en spawn.
        unsafe {
            self.tasks[t].assume_init_mut().priority = p;
        }
    }

    /// Conmuta a otras tareas hasta que la tarea `me` vuelva a estar `Ready`.
    ///
    /// Modela el bloqueo en un objeto de sincronización: igual que [`Self::sleep_ms`]
    /// pero el despertar lo provoca un unlock/post (no el reloj). Cada iteración
    /// evalúa el scheduler con IRQs enmascaradas (excluye la preempción) y, si no
    /// hay otra tarea lista, espera con `wfi`.
    fn switch_until_ready(&mut self, me: usize) {
        loop {
            let guard = A::enter_critical();
            if self.task_ref(me).state == TaskState::Ready {
                A::exit_critical(guard);
                return;
            }
            let next = self.pick_next(me);
            if next != me {
                self.current = next;
                self.slice_ticks = 0;
                self.prepare_task_hw(next);
                // SAFETY: índices válidos y contextos inicializados.
                unsafe {
                    let prev = &mut self.tasks[me].assume_init_mut().context as *mut A::Context;
                    let nx = &self.task_ref(next).context as *const A::Context;
                    A::switch_context(prev, nx);
                }
                A::exit_critical(guard);
                // Reanudada más tarde: el tope del loop reevalúa el estado.
            } else {
                A::exit_critical(guard);
                A::wait_for_interrupt();
            }
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
            match self.task_ref(i).state {
                TaskState::Sleeping(wake_at) if now.wrapping_sub(wake_at) as i32 >= 0 => {
                    // SAFETY: i < count; slot inicializado en spawn.
                    unsafe {
                        self.tasks[i].assume_init_mut().state = TaskState::Ready;
                    }
                }
                // Bloqueo IPC con plazo: al vencer se quita de la lista de waiters
                // del canal y se marca lista. El `chan_recv`/`chan_send` que la
                // reanude reintenta la operación; al fallar y ver el plazo vencido
                // devuelve `Etimedout`.
                TaskState::BlockedRecv(chan) | TaskState::BlockedSend(chan) => {
                    if let Some(deadline) = self.task_ref(i).block_deadline {
                        if now.wrapping_sub(deadline) as i32 >= 0 {
                            let bit = 1u8 << i;
                            if let TaskState::BlockedRecv(_) = self.task_ref(i).state {
                                self.channels[chan as usize].recv_waiters &= !bit;
                            } else {
                                self.channels[chan as usize].send_waiters &= !bit;
                            }
                            // SAFETY: i < count; slot inicializado en spawn.
                            unsafe {
                                let slot = self.tasks[i].assume_init_mut();
                                slot.state = TaskState::Ready;
                                slot.block_deadline = None;
                            }
                        }
                    }
                }
                _ => {}
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

    /// Marca el scheduler como arrancado y elige la primera tarea SIN saltar a
    /// ella (no invoca [`Arch::start_first`], que nunca retorna). Habilita
    /// pruebas host de [`Self::yield_now`]/[`Self::preempt_tick`]/[`Self::sleep_ms`]
    /// con un `Arch` simulado cuyo `switch_context` es un no-op.
    ///
    /// Solo con la feature `test-util`; ausente en los builds embebidos.
    #[cfg(feature = "test-util")]
    pub fn force_start_for_test(&mut self) {
        if self.count == 0 {
            return;
        }
        self.started = true;
        self.current = self.pick_next(usize::MAX);
    }

    /// Fija la tarea en ejecución (sin conmutar contexto). Permite a los tests
    /// host simular qué tarea llama a lock/unlock sin un `Arch` real.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn set_current_for_test(&mut self, idx: usize) {
        if idx < self.count {
            self.current = idx;
        }
    }

    /// Ejecuta SOLO la contabilidad de [`Self::mutex_lock`] (adquirir o marcar
    /// bloqueada + heredar prioridad) sin el bucle de conmutación, que en el host
    /// (con `switch_context` no-op) no progresaría. Devuelve `true` si adquirió.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn mutex_acquire_for_test(&mut self, id: usize) -> bool {
        let me = self.current;
        self.mutex_acquire(id, me)
    }

    /// Dueño actual del mutex `id` (índice de tarea), o `None` si libre.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn mutex_owner_for_test(&self, id: usize) -> Option<u8> {
        self.mutexes[id].owner
    }

    /// `true` si la tarea `idx` está bloqueada esperando el mutex `id`.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn is_blocked_on_mutex_for_test(&self, idx: usize, id: usize) -> bool {
        idx < self.count && self.task_ref(idx).state == TaskState::BlockedMutex(id as u8)
    }

    /// Marca la tarea `idx` como muerta (`Killed`) sin pasar por el camino de
    /// fault (que termina en [`Arch::resume_after_fault`], que nunca retorna).
    /// Permite a las pruebas host ejercitar [`Self::respawn`] y los contadores de
    /// salud sin un `Arch` real.
    ///
    /// Solo con la feature `test-util`; ausente en los builds embebidos.
    #[cfg(feature = "test-util")]
    pub fn mark_killed_for_test(&mut self, idx: usize) {
        if idx < self.count {
            // SAFETY: idx < count; slot inicializado en spawn.
            unsafe {
                self.tasks[idx].assume_init_mut().state = TaskState::Killed;
            }
        }
    }

    /// Número de mensajes en vuelo en el canal `chan`.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn chan_len_for_test(&self, chan: usize) -> usize {
        self.channels[chan].len as usize
    }

    /// `true` si la tarea `idx` está bloqueada esperando recibir del canal `chan`.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn is_blocked_on_recv_for_test(&self, idx: usize, chan: usize) -> bool {
        idx < self.count && self.task_ref(idx).state == TaskState::BlockedRecv(chan as u8)
    }

    /// `true` si la tarea `idx` está bloqueada esperando enviar al canal `chan`.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn is_blocked_on_send_for_test(&self, idx: usize, chan: usize) -> bool {
        idx < self.count && self.task_ref(idx).state == TaskState::BlockedSend(chan as u8)
    }

    /// Bloquea la tarea actual esperando recibir del canal `chan` con plazo
    /// absoluto `deadline` (solo contabilidad, sin el bucle de conmutación que en
    /// el host no progresaría). Permite probar el vencimiento por timeout.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn block_recv_for_test(&mut self, chan: usize, deadline: u32) {
        let me = self.current;
        self.channels[chan].recv_waiters |= 1 << me;
        // SAFETY: me < count; slot inicializado en spawn.
        unsafe {
            let slot = self.tasks[me].assume_init_mut();
            slot.state = TaskState::BlockedRecv(chan as u8);
            slot.block_deadline = Some(deadline);
        }
    }

    /// Ejecuta el barrido de despertar por plazo vencido (`Sleeping` y bloqueos
    /// IPC con `timeout`). Expone la lógica que [`Self::pick_next`] corre en cada
    /// elección, para probar el vencimiento sin un `Arch` real.
    ///
    /// Solo con la feature `test-util`.
    #[cfg(feature = "test-util")]
    pub fn wake_expired_for_test(&mut self) {
        self.wake_expired();
    }

    /// Plazo absoluto de liveness de `idx` (`None` si no está monitorizada).
    #[cfg(feature = "test-util")]
    pub fn liveness_deadline_for_test(&self, idx: usize) -> Option<u32> {
        self.task_ref(idx).liveness_deadline
    }

    /// `true` si la tarea `idx` está en estado `Killed`.
    #[cfg(feature = "test-util")]
    pub fn is_killed_for_test(&self, idx: usize) -> bool {
        self.task_ref(idx).state == TaskState::Killed
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
