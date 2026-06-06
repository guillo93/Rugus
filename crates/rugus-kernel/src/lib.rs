//! Capa de kernel cohesiva de Rugus para Cortex-M (F4/F7).
//!
//! Encima de `rugus-core` (mecanismo: scheduler, syscalls, fault) y
//! `rugus-arch-cortex-m` (hardware: context switch, MPU, SVC, SysTick), esta
//! capa cablea el kernel en un todo usable: **posee** el scheduler, registra de
//! una vez los hooks de syscall y el hook de fault, y expone un flujo de
//! arranque claro (`spawn` → `start`).
//!
//! ## Por qué un crate aparte
//!
//! Antes, cada `main` de placa repetía ~60 líneas de cableado idéntico (statics
//! `SCHEDULER`, `addr_of_mut!`, registro de `Hooks`, hook de fault). Esa
//! duplicación era frágil: un error en una placa no se reflejaba en la otra.
//! Aquí el cableado vive una sola vez y cada placa solo aporta lo que de verdad
//! es específico: relojes, MPU layout, periféricos y las tareas.
//!
//! ## Frontera de capas
//!
//! - `rugus-core` se mantiene LOG-FREE (TCB). El logging del fault vive AQUÍ
//!   (feature `defmt`), no en el core.
//! - La observabilidad de plataforma (LEDs, post-mortem) entra por
//!   [`FaultObserver`], que la placa registra en [`install`]: el kernel llama al
//!   observer ANTES de matar la tarea, sin acoplarse a periféricos concretos.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod console;
pub mod status;

use core::ptr::{addr_of, addr_of_mut};

use rugus_arch_cortex_m::{set_fault_hook, time, CortexM};
use rugus_core::channel::Channel;
use rugus_core::fault::FaultReport;
use rugus_core::sched::{Priority, Scheduler, SpawnError, TaskId};
use rugus_core::syscall::{self, Hooks};
use rugus_core::telemetry::FaultTelemetry;
use rugus_core::{Domain, Errno};

/// Tipo concreto del scheduler de esta capa (Arch fijado a Cortex-M).
type Sched = Scheduler<CortexM>;

/// Observador de faults de plataforma: el kernel lo invoca con el
/// [`FaultReport`] ANTES de matar la tarea culpable. Pensado para efectos de
/// plataforma (encender un LED de fault, grabar post-mortem); no debe retornar
/// trabajo al kernel ni asumir que la tarea sigue viva al volver.
pub type FaultObserver = fn(&FaultReport);

/// Único scheduler del binario. Cooperativo: sin reentrada concurrente.
static mut SCHEDULER: Sched = Sched::new();
/// Observador de fault registrado por la placa (opcional).
static mut FAULT_OBSERVER: Option<FaultObserver> = None;

/// Telemetría de faults persistente entre resets (F4.4).
///
/// Vive en la sección `.uninit` de `cortex-m-rt`: el runtime NO la pone a cero al
/// arrancar, así que su contenido (contadores, último post-mortem, conteo de
/// arranques) SOBREVIVE a un reset por watchdog o por fault. La validez se decide
/// por el `magic` en [`telemetry_init`]: arranque en frío (basura) reinicia,
/// reset en caliente preserva el historial. No tiene inicializador const porque
/// `.uninit` no se inicializa; solo se toca tras [`telemetry_init`].
#[link_section = ".uninit.RUGUS_FAULT_TELEMETRY"]
static mut FAULT_TELEMETRY: core::mem::MaybeUninit<FaultTelemetry> =
    core::mem::MaybeUninit::uninit();

/// Canal IPC único (id 0) por el que userland envía peticiones de I/O por valor
/// a un driver privilegiado. SPSC: el productor es el dispatch del syscall (una
/// app a la vez bajo el scheduler cooperativo), el consumidor es la tarea-driver
/// que llama a [`ipc_try_recv`]. Capacidad útil 7.
static IPC_MAILBOX: Channel<u32, 8> = Channel::new();

/// Cablea el kernel: registra los hooks de syscall y el hook de fault.
///
/// Llamar UNA vez desde `main`, después de `platform_init` (que habilita los
/// handlers de fault y la MPU) y antes de [`spawn`]/[`start`]. `observer` recibe
/// cada fault contenido para efectos de plataforma (LED, post-mortem); `None`
/// si la placa no necesita observabilidad extra (el kernel ya loguea con la
/// feature `defmt`).
///
/// # Safety
///
/// Solo desde `main`, en arranque single-thread, antes de lanzar tareas.
pub unsafe fn install(observer: Option<FaultObserver>) {
    unsafe {
        FAULT_OBSERVER = observer;
        set_fault_hook(fault_hook);
        // Preempción: la ISR de SysTick llamará a este trampolín cada tick para
        // que el scheduler reparta el CPU por rodajas (time-slice) sin depender
        // de que las tareas cedan voluntariamente.
        time::set_preempt_hook(preempt_tick);
        syscall::register(Hooks {
            yield_now,
            sleep_ms,
            current_task_id,
            current_domain,
            current_user_region,
            ipc_send,
            mutex_lock,
            mutex_unlock,
            sem_wait,
            sem_post,
            chan_send,
            chan_recv,
            checkin,
        });
    }
}

/// Inicializa la telemetría de faults persistente (F4.4) y devuelve `true` si
/// fue un **reset en caliente** (datos previos preservados) o `false` si fue un
/// **arranque en frío** (contadores reiniciados).
///
/// Llamar UNA vez desde `main`, en arranque temprano (antes de `spawn`/`start`).
/// Valida el `magic` de la región `.uninit`: como esa RAM puede contener basura
/// tras un power-on, esta función SOLO lee `magic` (un `u32`, cualquier patrón es
/// válido de leer) antes de decidir; nunca interpreta campos sin sellar.
///
/// # Safety
///
/// Solo desde `main`, single-thread, una vez. Sella la región `.uninit`.
pub unsafe fn telemetry_init() -> bool {
    // SAFETY: arranque single-thread; `boot()` solo lee `magic` (válido de leer
    // sobre cualquier patrón de bits) antes de sellar/reiniciar el resto.
    unsafe { (*addr_of_mut!(FAULT_TELEMETRY)).assume_init_mut().boot() }
}

/// Número de arranques observados desde el último arranque en frío (incluye el
/// actual). Llamar tras [`telemetry_init`].
pub fn boot_count() -> u32 {
    telemetry_ref().boot_count
}

/// Faults totales acumulados entre resets. Llamar tras [`telemetry_init`].
pub fn total_faults() -> u32 {
    telemetry_ref().total_faults
}

/// Faults contabilizados para la tarea `idx`. Llamar tras [`telemetry_init`].
pub fn faults_for(idx: usize) -> u32 {
    telemetry_ref().faults_for(idx)
}

/// `true` si el sistema debe entrar en safe-mode (demasiados faults totales o una
/// tarea reincidente). El supervisor lo consulta para dejar de respawnear y
/// degradarse de forma controlada en lugar de entrar en bucle de crash/respawn.
pub fn safe_mode() -> bool {
    telemetry_ref().safe_mode()
}

/// Último post-mortem registrado: `(kind, task_id, pc, addr)`, o `None` si no ha
/// habido ningún fault. Pensado para volcarlo por log al arrancar.
pub fn last_fault() -> Option<(u8, u8, u32, u32)> {
    let t = telemetry_ref();
    if t.has_last {
        Some((t.last_kind, t.last_task, t.last_pc, t.last_addr))
    } else {
        None
    }
}

#[inline]
fn telemetry_ref() -> &'static FaultTelemetry {
    // SAFETY: sellada por `telemetry_init` antes de cualquier lectura; lecturas
    // en cooperativo.
    unsafe { (*addr_of!(FAULT_TELEMETRY)).assume_init_ref() }
}

/// Nombre de la causa del último reset (la placa la lee de su RCC_CSR y la
/// publica aquí). El kernel no conoce el periférico: solo guarda el `&str` para
/// que la consola y el log de arranque lo muestren.
static mut RESET_CAUSE: &str = "?";

/// Publica la causa del último reset (F4.6). La placa la obtiene de su HAL
/// (`reset::read_and_clear().name()`) al arrancar.
///
/// # Safety
/// Llamar una sola vez en el arranque single-thread, antes de `start`.
pub unsafe fn set_reset_cause(name: &'static str) {
    // SAFETY: escritura única en arranque cooperativo single-thread.
    unsafe {
        RESET_CAUSE = name;
    }
}

/// Causa del último reset publicada por la placa, o `"?"` si no se fijó.
pub fn reset_cause() -> &'static str {
    // SAFETY: se fija una vez al arranque; lecturas cooperativas posteriores.
    unsafe { RESET_CAUSE }
}

/// Trampolín de preempción invocado por la ISR de SysTick: rutea al scheduler.
fn preempt_tick() {
    // SAFETY: corre en la ISR de SysTick; el modo hilo enmascara SysTick
    // mientras toca el scheduler, así que no hay reentrada concurrente.
    unsafe { scheduler_mut().preempt_tick() }
}

/// Registra una tarea privilegiada (kernel/driver) con su pila estática.
///
/// # Safety
///
/// Solo desde `main`, antes de [`start`]; `stack` debe vivir tanto como el
/// kernel (típicamente un `static mut`).
pub unsafe fn spawn(
    stack: &'static mut [u8],
    entry: fn() -> !,
    priority: Priority,
) -> Result<TaskId, SpawnError> {
    unsafe { scheduler_mut().spawn(stack, entry, priority) }
}

/// Registra una app userland (nPRIV + dominio App + región MPU dedicada).
///
/// # Safety
///
/// Igual que [`spawn`]; además `stack` debe cumplir las restricciones de la
/// región MPU App-RW (potencia de 2, alineada) — el scheduler las verifica.
pub unsafe fn spawn_user(
    stack: &'static mut [u8],
    entry: fn() -> !,
    priority: Priority,
) -> Result<TaskId, SpawnError> {
    unsafe { scheduler_mut().spawn_user(stack, entry, priority) }
}

/// Arranca el scheduler; no retorna. La placa ya debe haber hecho `spawn`.
pub fn start() -> ! {
    // SAFETY: scheduler poseído por esta capa; arranque single-thread.
    unsafe { scheduler_mut().start() }
}

/// Cede el CPU desde una tarea PRIVILEGIADA del kernel (que no usa el
/// trampolín SVC de userland). Cooperativo.
pub fn cpu_yield() {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().yield_now() }
}

/// Duerme `ms` la tarea privilegiada actual cediendo el CPU (sin busy-wait).
pub fn cpu_sleep_ms(ms: u32) {
    // SAFETY: igual que cpu_yield.
    unsafe { scheduler_mut().sleep_ms(ms) }
}

/// Toma el mutex `id` desde una tarea PRIVILEGIADA (bloquea con herencia de
/// prioridad si está ocupado). Cooperativo.
pub fn cpu_mutex_lock(id: usize) -> i32 {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().mutex_lock(id) }
}

/// Libera el mutex `id` desde una tarea privilegiada.
pub fn cpu_mutex_unlock(id: usize) -> i32 {
    // SAFETY: igual que cpu_mutex_lock.
    unsafe { scheduler_mut().mutex_unlock(id) }
}

/// Consume un permiso del semáforo `id` desde una tarea privilegiada (bloquea
/// si no hay).
pub fn cpu_sem_wait(id: usize) -> i32 {
    // SAFETY: igual que cpu_mutex_lock.
    unsafe { scheduler_mut().sem_wait(id) }
}

/// Devuelve un permiso al semáforo `id` desde una tarea privilegiada.
pub fn cpu_sem_post(id: usize) -> i32 {
    // SAFETY: igual que cpu_mutex_lock.
    unsafe { scheduler_mut().sem_post(id) }
}

/// Envía `msg` por el canal IPC `chan` desde una tarea privilegiada, con
/// `timeout_ms` (`0` no bloquea; `u32::MAX` indefinido). Cooperativo.
pub fn cpu_chan_send(chan: usize, msg: u32, timeout_ms: u32) -> i32 {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().chan_send(chan, msg, timeout_ms) }
}

/// Recibe del canal IPC `chan` desde una tarea privilegiada, escribiendo el
/// mensaje en `out`. Bloquea hasta `timeout_ms` ms si está vacío.
pub fn cpu_chan_recv(chan: usize, timeout_ms: u32, out: &mut u32) -> i32 {
    // SAFETY: igual que cpu_chan_send.
    unsafe { scheduler_mut().chan_recv(chan, timeout_ms, out) }
}

/// Bloquea en la variable de condición `cv` liberando el mutex `mtx` (que la
/// tarea privilegiada debe poseer) y lo re-adquiere al despertar. `timeout_ms`:
/// `0` no bloquea, `u32::MAX` indefinido. Patrón canónico:
/// `cpu_mutex_lock(m); while !cond { cpu_condvar_wait(c, m, t); } cpu_mutex_unlock(m);`.
pub fn cpu_condvar_wait(cv: usize, mtx: usize, timeout_ms: u32) -> i32 {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().condvar_wait(cv, mtx, timeout_ms) }
}

/// Despierta al waiter de mayor prioridad bloqueado en la condvar `cv`.
pub fn cpu_condvar_signal(cv: usize) -> i32 {
    // SAFETY: igual que cpu_condvar_wait.
    unsafe { scheduler_mut().condvar_signal(cv) }
}

/// Despierta a TODAS las tareas bloqueadas en la condvar `cv`.
pub fn cpu_condvar_broadcast(cv: usize) -> i32 {
    // SAFETY: igual que cpu_condvar_wait.
    unsafe { scheduler_mut().condvar_broadcast(cv) }
}

/// Configura la barrera `id` para que abra al converger `threshold` tareas.
/// Llamar desde `main` antes de [`start`].
///
/// # Safety
///
/// Solo desde `main`, single-thread, antes de lanzar tareas.
pub unsafe fn barrier_init(id: usize, threshold: u32) {
    // SAFETY: arranque single-thread garantizado por el caller.
    unsafe { scheduler_mut().barrier_init(id, threshold) }
}

/// Espera en la barrera `id` desde una tarea privilegiada: bloquea hasta que
/// converjan `threshold` tareas. Cooperativo.
pub fn cpu_barrier_wait(id: usize) -> i32 {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().barrier_wait(id) }
}

/// Fija (OR) `bits` en el grupo de eventos `id` y despierta a las tareas cuya
/// espera quede satisfecha.
pub fn cpu_event_set(id: usize, bits: u32) -> i32 {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().event_set(id, bits) }
}

/// Limpia (AND-NOT) `bits` del grupo de eventos `id`.
pub fn cpu_event_clear(id: usize, bits: u32) -> i32 {
    // SAFETY: igual que cpu_event_set.
    unsafe { scheduler_mut().event_clear(id, bits) }
}

/// Bits actualmente fijados en el grupo de eventos `id`.
pub fn cpu_event_get(id: usize) -> u32 {
    // SAFETY: igual que cpu_event_set.
    unsafe { scheduler_mut().event_get(id) }
}

/// Espera bits en el grupo de eventos `id` desde una tarea privilegiada.
/// `wait_all`: todos los bits de `mask` (`true`) o cualquiera (`false`).
/// `timeout_ms`: `0` no bloquea, `u32::MAX` indefinido. Cooperativo.
pub fn cpu_event_wait(id: usize, mask: u32, wait_all: bool, timeout_ms: u32) -> i32 {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().event_wait(id, mask, wait_all, timeout_ms) }
}

/// Nº de deadlocks (ciclos de espera de mutex) detectados desde el arranque.
///
/// El detector (F5.D.3) anota cada vez que una toma de mutex cierra un ciclo en
/// el grafo de espera `tarea`→`mutex`→`dueño`. No aborta: el supervisor puede
/// vigilar este contador para registrar, alertar o autorreparar.
pub fn deadlock_count() -> u32 {
    scheduler_ref().deadlock_count()
}

/// Última arista `(tarea, mutex)` que cerró un ciclo de espera, o `None`.
pub fn last_deadlock() -> Option<(u8, u8)> {
    scheduler_ref().last_deadlock()
}

/// Arma la monitorización de liveness de la tarea `idx`: debe emitir un
/// `checkin` cada `period_ms` ms como máximo o el supervisor la considerará
/// colgada. Llamar desde `main` o desde el supervisor.
///
/// # Safety
///
/// Solo desde contexto privilegiado (main o supervisor), sin reentrada.
pub unsafe fn set_liveness_period(idx: usize, period_ms: u32) {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().set_liveness_period(idx, period_ms) }
}

/// Renueva el plazo de liveness de la tarea `idx` (latido por proxy desde una
/// tarea privilegiada). La ruta userland es el syscall `Checkin`.
pub fn cpu_checkin(idx: usize) {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().liveness_checkin(idx) }
}

/// Índice de la primera tarea cuyo plazo de liveness venció (colgada pero viva),
/// o `None`. El supervisor lo consulta para recuperar tareas atascadas.
pub fn liveness_overdue() -> Option<usize> {
    scheduler_ref().liveness_overdue()
}

/// Mata por la fuerza la tarea viva `idx` (no la actual) para recuperarla;
/// `true` si la mató. El supervisor la combina con [`respawn`] para reiniciar en
/// frío una tarea colgada que el fault containment no captura.
pub fn force_kill(idx: usize) -> bool {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().force_kill(idx) }
}

/// Inicializa el semáforo `id` con `count` permisos. Llamar desde `main` antes
/// de [`start`].
///
/// # Safety
///
/// Solo desde `main`, arranque single-thread, antes de lanzar tareas.
pub unsafe fn sem_init(id: usize, count: u32) {
    // SAFETY: arranque single-thread garantizado por el caller.
    unsafe { scheduler_mut().sem_init(id, count) }
}

/// Autotest determinista (no bloqueante) de la sincronización del kernel.
///
/// Llamar desde `main` DESPUÉS de [`spawn`] (para que exista la tarea 0) y ANTES
/// de [`start`]: con el scheduler aún sin arrancar, lock/wait degradan a su forma
/// no bloqueante, así que el test verifica la contabilidad de mutex y semáforo
/// sin conmutar. La corrección de la herencia de prioridad y el bloqueo se cubre
/// en `rugus-host-tests`. Devuelve `true` si todos los invariantes se cumplen.
///
/// # Safety
///
/// Solo desde `main`, single-thread, tras al menos un [`spawn`].
pub unsafe fn sync_selftest() -> bool {
    // SAFETY: arranque single-thread; tarea 0 ya registrada.
    let s = unsafe { scheduler_mut() };
    let mut ok = true;
    // Mutex 0 libre → se toma; re-lock por el dueño es no-op exitoso.
    ok &= s.mutex_try_lock(0);
    ok &= s.mutex_try_lock(0);
    // Liberar deja el mutex libre; un segundo unlock por quien no es dueño falla.
    ok &= s.mutex_unlock(0) == 0;
    ok &= s.mutex_unlock(0) == Errno::Edenied as i32;
    // Semáforo 0 con 2 permisos: dos waits pasan, el tercero no; tras post vuelve.
    s.sem_init(0, 2);
    ok &= s.sem_try_wait(0);
    ok &= s.sem_try_wait(0);
    ok &= !s.sem_try_wait(0);
    ok &= s.sem_post(0) == 0;
    ok &= s.sem_try_wait(0);
    // Restaura el semáforo a 0 para que el uso real arranque limpio.
    s.sem_init(0, 0);
    // Canal IPC (sin arrancar → no bloqueante): send encola, recv desencola FIFO.
    let mut got = 0u32;
    ok &= s.chan_send(0, 0xC0FFEE, 0) == 0;
    ok &= s.chan_send(0, 0xBEEF, 0) == 0;
    ok &= s.chan_recv(0, 0, &mut got) == 0 && got == 0xC0FFEE;
    ok &= s.chan_recv(0, 0, &mut got) == 0 && got == 0xBEEF;
    // Canal vacío sin bloquear → Ebusy.
    ok &= s.chan_recv(0, 0, &mut got) == Errno::Ebusy as i32;
    // Variables de condición (F5.D.1), rutas no bloqueantes verificables sin
    // arrancar: ids fuera de rango → Einval; signal/broadcast sobre una condvar
    // vacía → 0 (no-op); con el scheduler aún parado, condvar_wait degrada a
    // Ebusy sin intentar bloquear. La semántica de bloqueo/señal/timeout real se
    // cubre en `rugus-host-tests`.
    ok &= s.condvar_signal(99) == Errno::Einval as i32;
    ok &= s.condvar_broadcast(99) == Errno::Einval as i32;
    ok &= s.condvar_wait(99, 0, 0) == Errno::Einval as i32;
    ok &= s.condvar_signal(0) == 0;
    ok &= s.condvar_broadcast(0) == 0;
    ok &= s.condvar_wait(0, 0, 0) == Errno::Ebusy as i32;
    // Barreras y grupos de eventos (F5.D.2), rutas no bloqueantes verificables sin
    // arrancar: id fuera de rango → Einval; barrera sin configurar (threshold 0) →
    // Einval. Con threshold válido pero el scheduler aún parado, barrier_wait degrada
    // a Ebusy. Los eventos validan id, y un roundtrip set→get→clear comprueba la
    // máscara de bits. La semántica de bloqueo/apertura/timeout real se cubre en
    // `rugus-host-tests`.
    ok &= s.barrier_wait(99) == Errno::Einval as i32;
    ok &= s.barrier_wait(0) == Errno::Einval as i32; // sin configurar
    s.barrier_init(0, 2);
    ok &= s.barrier_wait(0) == Errno::Ebusy as i32; // configurada pero sin start
    s.barrier_init(0, 0); // restaura desconfigurada para uso real
    ok &= s.event_set(99, 1) == Errno::Einval as i32;
    ok &= s.event_wait(99, 1, false, 0) == Errno::Einval as i32;
    ok &= s.event_clear(99, 1) == Errno::Einval as i32;
    ok &= s.event_set(0, 0b1010) == 0;
    ok &= s.event_get(0) == 0b1010;
    ok &= s.event_wait(0, 0b0010, false, 0) == 0; // ya satisfecho → no bloquea
    ok &= s.event_wait(0, 0b0100, false, 0) == Errno::Ebusy as i32; // no satisfecho
    ok &= s.event_clear(0, 0b1010) == 0;
    ok &= s.event_get(0) == 0; // limpio para el uso real
    ok
}

/// Autotest determinista del monitor de liveness (F4.3).
///
/// Llamar desde `main` DESPUÉS de [`spawn`] (tarea 0 registrada) y ANTES de
/// [`start`]. Con el reloj aún en 0 (SysTick sin arrancar), arma un periodo en la
/// tarea 0 y verifica que el monitor NO la declara colgada de inmediato (plazo en
/// el futuro). No avanza el reloj (no puede sin SysTick), así que la detección de
/// vencimiento real se cubre en `rugus-host-tests` con reloj controlable. Deja la
/// liveness desarmada al terminar. Devuelve `true` si los invariantes se cumplen.
///
/// # Safety
///
/// Solo desde `main`, single-thread, tras al menos un [`spawn`].
pub unsafe fn liveness_selftest() -> bool {
    // SAFETY: arranque single-thread; tarea 0 ya registrada.
    let s = unsafe { scheduler_mut() };
    let mut ok = true;
    // Sin liveness armada, ninguna tarea está colgada.
    ok &= s.liveness_overdue().is_none();
    // Armar un periodo amplio fija el plazo en el futuro: no vencido todavía.
    s.set_liveness_period(0, 60_000);
    // La tarea 0 es la actual (current==0 antes de start) → liveness_overdue la
    // excluye por definición; el invariante observable es simplemente que no
    // explota y que un checkin renueva sin error.
    s.liveness_checkin(0);
    ok &= s.liveness_overdue().is_none();
    // Restaura: rearmará el periodo real cada app/tarea cuando corresponda.
    s.set_liveness_period(0, 0);
    ok
}

/// `true` si la tarea `idx` fue matada por un fault contenido.
pub fn task_killed(idx: usize) -> bool {
    scheduler_ref().is_killed(idx)
}

/// Número de tareas registradas.
pub fn task_count() -> usize {
    scheduler_ref().task_count()
}

/// Número de tareas que un fault mató (indicador de salud del supervisor).
pub fn killed_count() -> usize {
    scheduler_ref().killed_count()
}

/// Milisegundos hasta el próximo despertar por tiempo (saturado a `0` si ya
/// venció), o `None` si ninguna tarea espera por reloj. Insumo del tick dinámico
/// (F5.A): la capa de tiempo del arch lo consulta antes de un `wfi` para decidir
/// cuánto puede dormir sin perder un plazo de `sleep`/IPC/condvar/evento.
pub fn next_wake_ms() -> Option<u32> {
    scheduler_ref().next_wake_ms()
}

/// Revive una tarea `Killed` reconstruyendo su frame inicial; `true` si lo hizo.
///
/// La invoca el supervisor privilegiado para autorreparar una app caída: arranca
/// limpia desde su `entry` original. No-op si `idx` no está matada.
pub fn respawn(idx: usize) -> bool {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().respawn(idx) }
}

/// Uso máximo de pila (high-water) y total de la tarea `idx`, en bytes.
pub fn stack_usage(idx: usize) -> (u32, u32) {
    let s = scheduler_ref();
    (s.stack_high_water(idx), s.stack_len(idx))
}

/// Uso máximo de pila (high-water) de la tarea `idx`, en bytes.
pub fn stack_high_water(idx: usize) -> u32 {
    scheduler_ref().stack_high_water(idx)
}

/// Tamaño total de la pila de la tarea `idx`, en bytes.
pub fn stack_len(idx: usize) -> u32 {
    scheduler_ref().stack_len(idx)
}

/// Prioridad efectiva actual de la tarea `idx` (0 = más alta).
pub fn task_priority(idx: usize) -> u8 {
    scheduler_ref().task_priority(idx)
}

/// `true` si la tarea `idx` es userland (nPRIV + sandbox MPU).
pub fn is_user_task(idx: usize) -> bool {
    scheduler_ref().is_user_task(idx)
}

/// Etiqueta legible del estado de la tarea `idx` (`READY`/`SLEEP`/`KILL`/…).
pub fn task_state_name(idx: usize) -> &'static str {
    scheduler_ref().task_state_name(idx)
}

/// Bytes asignados en el heap del sistema (0 si el binario no tiene heap).
#[cfg(feature = "alloc")]
pub fn heap_used() -> usize {
    rugus_core::heap::used()
}
/// Bytes asignados en el heap del sistema (0 si el binario no tiene heap).
#[cfg(not(feature = "alloc"))]
pub fn heap_used() -> usize {
    0
}

/// Bytes libres en el heap del sistema (0 si el binario no tiene heap).
#[cfg(feature = "alloc")]
pub fn heap_free() -> usize {
    rugus_core::heap::free()
}
/// Bytes libres en el heap del sistema (0 si el binario no tiene heap).
#[cfg(not(feature = "alloc"))]
pub fn heap_free() -> usize {
    0
}

/// Tamaño total del heap del sistema (0 si el binario no tiene heap).
#[cfg(feature = "alloc")]
pub fn heap_size() -> usize {
    rugus_core::heap::size()
}
/// Tamaño total del heap del sistema (0 si el binario no tiene heap).
#[cfg(not(feature = "alloc"))]
pub fn heap_size() -> usize {
    0
}

/// Reinicia el sistema por software (`SCB.AIRCR.SYSRESETREQ`). No retorna. La
/// telemetría persistente en `.uninit` sobrevive a este reset (warm boot).
pub fn reboot() -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}

/// Saca la siguiente petición IPC del buzón userland, o `None` si está vacío.
///
/// La consume la tarea-driver privilegiada (único consumidor del SPSC). El
/// `msg` es opaco a esta capa: lo interpreta el driver de la placa.
pub fn ipc_try_recv() -> Option<u32> {
    IPC_MAILBOX.try_recv()
}

// --- Hooks de syscall: rutean al scheduler poseído por la capa. ---

/// Hook de `Id::IpcSend`: encola `msg` en el buzón del kernel. Solo el canal 0
/// existe por ahora; cualquier otro id devuelve `Einval`. `Ebusy` si está lleno.
fn ipc_send(chan: u32, msg: u32) -> i32 {
    if chan != 0 {
        return Errno::Einval as i32;
    }
    match IPC_MAILBOX.try_send(msg) {
        Ok(()) => 0,
        Err(_) => Errno::Ebusy as i32,
    }
}

/// Hook de `Id::MutexLock`: toma el mutex `id` (bloquea con herencia de
/// prioridad si está ocupado).
fn mutex_lock(id: u32) -> i32 {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().mutex_lock(id as usize) }
}

/// Hook de `Id::MutexUnlock`: libera el mutex `id`.
fn mutex_unlock(id: u32) -> i32 {
    // SAFETY: igual que mutex_lock.
    unsafe { scheduler_mut().mutex_unlock(id as usize) }
}

/// Hook de `Id::SemWait`: consume un permiso del semáforo `id` (bloquea si no hay).
fn sem_wait(id: u32) -> i32 {
    // SAFETY: igual que mutex_lock.
    unsafe { scheduler_mut().sem_wait(id as usize) }
}

/// Hook de `Id::SemPost`: devuelve un permiso al semáforo `id`.
fn sem_post(id: u32) -> i32 {
    // SAFETY: igual que mutex_lock.
    unsafe { scheduler_mut().sem_post(id as usize) }
}

/// Hook de `Id::ChanSend`: envía `msg` por el canal `chan` con `timeout_ms`.
fn chan_send(chan: u32, msg: u32, timeout_ms: u32) -> i32 {
    // SAFETY: igual que mutex_lock.
    unsafe { scheduler_mut().chan_send(chan as usize, msg, timeout_ms) }
}

/// Hook de `Id::ChanRecv`: recibe del canal `chan` con `timeout_ms` y escribe el
/// mensaje en `out_ptr` (rango ya validado por el dispatch).
fn chan_recv(chan: u32, timeout_ms: u32, out_ptr: u32) -> i32 {
    let mut msg = 0u32;
    // SAFETY: igual que mutex_lock.
    let r = unsafe { scheduler_mut().chan_recv(chan as usize, timeout_ms, &mut msg) };
    if r == 0 {
        // SAFETY: el dispatch validó [out_ptr, out_ptr+4) contra la región del
        // llamante (o es privilegiado, confiado) antes de invocar este hook.
        unsafe {
            (out_ptr as *mut u32).write_volatile(msg);
        }
    }
    r
}

/// Hook de `Id::Checkin`: renueva el plazo de liveness de la tarea en ejecución.
fn checkin() {
    // SAFETY: igual que yield_now.
    unsafe { scheduler_mut().liveness_checkin_current() }
}

fn yield_now() {
    // SAFETY: scheduler poseído; cooperativo sin reentrada concurrente.
    unsafe { scheduler_mut().yield_now() }
}

fn sleep_ms(ms: u32) {
    // SAFETY: igual que yield_now.
    unsafe { scheduler_mut().sleep_ms(ms) }
}

fn current_task_id() -> TaskId {
    scheduler_ref().current_id()
}

fn current_domain() -> Domain {
    scheduler_ref().current_domain()
}

fn current_user_region() -> Option<(u32, u32)> {
    scheduler_ref().current_user_region()
}

/// Política de fault del kernel: loguea (feature `defmt`), avisa al observer de
/// plataforma y mata SOLO la tarea culpable, reanudando la siguiente. No hay
/// panic global: si no quedan tareas vivas, el scheduler hace WFI.
fn fault_hook(report: FaultReport) -> ! {
    #[cfg(feature = "defmt")]
    defmt::error!(
        "task fault {} domain={} pc={=u32:#x} addr={=u32:#x} task={=u8} -> kill+resume",
        report.kind.name(),
        report.domain.name(),
        report.pc,
        report.addr.unwrap_or(0),
        report.task_id.0
    );
    // Latch de fault del servicio de estado: el LED de fault lo refleja cualquier
    // placa vía `status::refresh`, sin registrar un observer solo para eso.
    status::latch_fault();
    // SAFETY: contexto de fault (handler mode), single-thread; observer y
    // scheduler registrados en `install`.
    unsafe {
        // Post-mortem persistente: contabiliza el fault en la telemetría `.uninit`
        // ANTES de matar la tarea, para que sobreviva incluso si el siguiente paso
        // acaba en reset por watchdog.
        (*addr_of_mut!(FAULT_TELEMETRY))
            .assume_init_mut()
            .record(&report);
        if let Some(observer) = FAULT_OBSERVER {
            observer(&report);
        }
        scheduler_mut().kill_current_and_resume(report)
    }
}

#[inline]
fn scheduler_ref() -> &'static Sched {
    // SAFETY: scheduler inicializado const; lecturas en cooperativo.
    unsafe { &*addr_of!(SCHEDULER) }
}

#[inline]
#[allow(clippy::mut_from_ref)]
unsafe fn scheduler_mut() -> &'static mut Sched {
    // SAFETY: caller garantiza ausencia de reentrada concurrente (cooperativo).
    unsafe { &mut *addr_of_mut!(SCHEDULER) }
}
