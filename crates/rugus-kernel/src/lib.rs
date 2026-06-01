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

use core::ptr::{addr_of, addr_of_mut};

use rugus_arch_cortex_m::{set_fault_hook, CortexM};
use rugus_core::channel::Channel;
use rugus_core::fault::FaultReport;
use rugus_core::sched::{Priority, Scheduler, SpawnError, TaskId};
use rugus_core::syscall::{self, Hooks};
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
        syscall::register(Hooks {
            yield_now,
            sleep_ms,
            current_task_id,
            current_domain,
            current_user_region,
            ipc_send,
        });
    }
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

/// Uso máximo de pila (high-water) y total de la tarea `idx`, en bytes.
pub fn stack_usage(idx: usize) -> (u32, u32) {
    let s = scheduler_ref();
    (s.stack_high_water(idx), s.stack_len(idx))
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
    // SAFETY: contexto de fault (handler mode), single-thread; observer y
    // scheduler registrados en `install`.
    unsafe {
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
