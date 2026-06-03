//! Tests host de la lógica arch-agnóstica de `rugus-core`.
//!
//! Estos tests corren en el triple nativo del PC (no en ARM): ejercitan el
//! scheduler, el invariante del sandbox MPU (alineación de stacks userland) y el
//! ABI de syscalls SIN hardware, mediante un [`MockArch`] cuyo `switch_context`
//! es un no-op y cuyo reloj es controlable. Cierran la regresión de regla:
//! cualquier cambio en `rugus-core` se valida aquí en CI antes de tocar placa.
//!
//! El crate está EXCLUIDO del workspace embebido (igual que `rugus-proto`/
//! `rugus-cli`), así que su dependencia de `std` y la feature `test-util` de
//! `rugus-core` no se filtran a los binarios del kernel.

use rugus_core::arch::{Arch, CriticalGuard};
use rugus_core::sched::TaskMode;

use std::alloc::{alloc_zeroed, Layout};
use std::cell::Cell;

thread_local! {
    /// Reloj monotónico simulado (ms). Controlado por [`set_clock`].
    static CLOCK: Cell<u32> = const { Cell::new(0) };
    /// Nº de `switch_context` observados (para verificar conmutaciones).
    static SWITCHES: Cell<u32> = const { Cell::new(0) };
}

/// Pone el reloj simulado en `ms`.
pub fn set_clock(ms: u32) {
    CLOCK.with(|c| c.set(ms));
}

/// Conmutaciones de contexto observadas desde el último [`reset_mock`].
pub fn switches() -> u32 {
    SWITCHES.with(Cell::get)
}

/// Reinicia el estado por-hilo del mock (cargo reutiliza hilos entre tests).
pub fn reset_mock() {
    set_clock(0);
    SWITCHES.with(|c| c.set(0));
}

/// Contexto simulado: el scheduler solo lo pasa por puntero, nunca lo deref.
#[derive(Default)]
pub struct MockContext {
    _sp: u32,
}

/// Handle de sección crítica simulado (no enmascara nada en host).
pub struct MockGuard;
impl CriticalGuard for MockGuard {}

/// `Arch` simulado para tests host. `switch_context` cuenta llamadas; el resto
/// de primitivas son no-ops salvo `now_ms` (reloj controlable). Los métodos que
/// nunca retornan (`start_first`/`resume_after_fault`/`reset`) hacen `panic!`:
/// los tests usan `force_start_for_test`/`mark_killed_for_test` para no tomar
/// esos caminos.
pub struct MockArch;

impl Arch for MockArch {
    type Context = MockContext;
    type SavedIrq = MockGuard;

    const HAS_MEMORY_PROTECTION: bool = true;

    unsafe fn switch_context(_prev: *mut Self::Context, _next: *const Self::Context) {
        SWITCHES.with(|c| c.set(c.get() + 1));
    }

    fn init_task_stack(_stack: &mut [u8], _entry: fn() -> !, _privileged: bool) -> Self::Context {
        MockContext::default()
    }

    fn start_first(_ctx: *const Self::Context) -> ! {
        panic!("start_first no debe usarse en tests host (usa force_start_for_test)");
    }

    unsafe fn resume_after_fault(_ctx: *const Self::Context) -> ! {
        panic!("resume_after_fault no debe usarse en tests host");
    }

    fn on_task_switch(_mode: TaskMode, _stack_base: u32, _stack_len: u32) {}

    fn enter_critical() -> Self::SavedIrq {
        MockGuard
    }

    fn exit_critical(_saved: Self::SavedIrq) {}

    fn wait_for_interrupt() {}

    fn now_ms() -> u32 {
        CLOCK.with(Cell::get)
    }

    fn reset() -> ! {
        panic!("reset no debe usarse en tests host");
    }
}

/// Punto de entrada ficticio para tareas: el mock ignora la `entry` (no salta a
/// ella), así que nunca se ejecuta. `panic!` evita el lint de bucle vacío.
pub fn dummy_entry() -> ! {
    panic!("dummy_entry nunca se ejecuta en tests host");
}

/// Reserva un stack alineado a su tamaño (`size` debe ser potencia de 2), como
/// exige el sandbox MPU userland. Se filtra a propósito (vida `'static`): es un
/// test, no hay reclamo de memoria que validar.
pub fn aligned_stack(size: usize) -> &'static mut [u8] {
    assert!(size.is_power_of_two());
    let layout = Layout::from_size_align(size, size).unwrap();
    // SAFETY: layout válido (tamaño>0, align pot. de 2); puntero no nulo
    // comprobado; se expone como slice del tamaño exacto reservado.
    unsafe {
        let p = alloc_zeroed(layout);
        assert!(!p.is_null());
        std::slice::from_raw_parts_mut(p, size)
    }
}

/// Stack privilegiado: no exige alineación, basta un bloque de `size` bytes.
pub fn plain_stack(size: usize) -> &'static mut [u8] {
    Box::leak(vec![0u8; size].into_boxed_slice())
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    use rugus_core::sched::{Priority, Scheduler, SpawnError, MAX_TASKS};

    type Sched = Scheduler<MockArch>;

    #[test]
    fn spawn_rejects_small_stack() {
        let mut s = Sched::new();
        let stack = plain_stack(128);
        assert_eq!(
            s.spawn(stack, dummy_entry, Priority::Kernel),
            Err(SpawnError::StackTooSmall)
        );
    }

    #[test]
    fn spawn_user_rejects_unaligned_or_non_pow2_stack() {
        let mut s = Sched::new();
        // len = 300: ni potencia de 2 → rechazo del sandbox MPU.
        let stack = plain_stack(300);
        assert_eq!(
            s.spawn_user(stack, dummy_entry, Priority::App),
            Err(SpawnError::UnalignedUserStack)
        );
    }

    #[test]
    fn spawn_rejects_when_table_full() {
        let mut s = Sched::new();
        for _ in 0..MAX_TASKS {
            assert!(s
                .spawn(plain_stack(512), dummy_entry, Priority::Kernel)
                .is_ok());
        }
        assert_eq!(
            s.spawn(plain_stack(512), dummy_entry, Priority::Kernel),
            Err(SpawnError::TooManyTasks)
        );
    }

    #[test]
    fn round_robin_within_band() {
        let mut s = Sched::new();
        for _ in 0..3 {
            s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
                .unwrap();
        }
        s.force_start_for_test();
        // Secuencia determinista del cursor por banda: arranca en 1 y rota.
        let mut seq = vec![s.current_id().0];
        for _ in 0..3 {
            s.yield_now();
            seq.push(s.current_id().0);
        }
        assert_eq!(seq, vec![1, 2, 0, 1]);
    }

    #[test]
    fn higher_priority_band_preempts() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.spawn_user(aligned_stack(1024), dummy_entry, Priority::App)
            .unwrap(); // idx 1
        s.spawn_user(aligned_stack(1024), dummy_entry, Priority::App)
            .unwrap(); // idx 2
        s.force_start_for_test();
        // Kernel (banda superior) se elige primero.
        assert_eq!(s.current_id().0, 0);
        // Al ceder el kernel, corre una app...
        s.yield_now();
        assert_eq!(s.current_id().0, 1);
        // ...y al ceder la app, el kernel (listo) vuelve a ganar.
        s.yield_now();
        assert_eq!(s.current_id().0, 0);
    }

    #[test]
    fn preempt_tick_switches_only_at_slice_boundary() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        let start = s.current_id().0;
        // 9 ticks: la rodaja (SLICE_TICKS=10) aún no vence → sin conmutar.
        for _ in 0..9 {
            s.preempt_tick();
        }
        assert_eq!(s.current_id().0, start);
        // 10º tick: vence la rodaja → conmuta a la otra tarea.
        s.preempt_tick();
        assert_ne!(s.current_id().0, start);
    }

    #[test]
    fn sleep_then_wake_on_deadline() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 1
        reset_mock();
        s.force_start_for_test();
        let sleeper = s.current_id().0 as usize; // == 1
                                                 // Duerme la tarea actual 100 ms; otra tarea lista toma el CPU.
        s.sleep_ms(100);
        assert_ne!(s.current_id().0 as usize, sleeper);
        assert!(!s.task_alive(sleeper)); // durmiendo, no Ready
                                         // Antes del plazo no despierta.
        set_clock(50);
        s.yield_now();
        assert!(!s.task_alive(sleeper));
        // Al alcanzar el plazo, vuelve a estar lista y se elige.
        set_clock(100);
        s.yield_now();
        assert!(s.task_alive(sleeper));
    }

    #[test]
    fn kill_marks_state_and_counts() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        assert_eq!(s.killed_count(), 0);
        assert!(s.task_alive(0));
        s.mark_killed_for_test(0);
        assert!(s.is_killed(0));
        assert!(!s.task_alive(0));
        assert_eq!(s.killed_count(), 1);
    }

    #[test]
    fn respawn_guard_rejects_alive_and_out_of_range() {
        // El revive completo de `respawn` reconstruye un puntero al stack desde
        // `stack_base` (u32) y lo repinta; en el host de 64 bits ese puntero está
        // truncado, así que el repintado real solo se valida en placa (F3.6: 22
        // ciclos kill→respawn en F769). Aquí cubrimos la lógica de guarda, que
        // retorna ANTES de tocar memoria.
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        // Índice fuera de rango → false (sin tocar memoria).
        assert!(!s.respawn(99));
        // Tarea viva (no Killed) → false (sin tocar memoria).
        assert!(!s.respawn(0));
    }
}

#[cfg(test)]
mod sync_tests {
    //! Sincronización con herencia de prioridad (F4.1).
    //!
    //! `MockArch::switch_context` es un no-op, así que la API bloqueante
    //! (`mutex_lock`/`sem_wait`) entraría en bucle infinito en host. Por eso
    //! estos tests ejercen la contabilidad vía accesores `*_for_test` y la API
    //! no bloqueante (`try_lock`/`try_wait`/`post`/`unlock`).
    use super::*;
    use rugus_core::sched::{Priority, Scheduler};

    type Sched = Scheduler<MockArch>;

    #[test]
    fn priority_inheritance_boosts_and_restores() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::App).unwrap(); // idx 0 (baja)
        s.spawn(plain_stack(512), dummy_entry, Priority::Service)
            .unwrap(); // idx 1 (media)
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 2 (alta)
        s.force_start_for_test();

        // idx0 (App=2) toma el mutex 0.
        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0));
        assert_eq!(s.mutex_owner_for_test(0), Some(0));
        assert_eq!(s.task_priority(0), Priority::App as u8);

        // idx2 (Kernel=0) intenta el mutex 0 → se bloquea y eleva al dueño.
        s.set_current_for_test(2);
        assert!(!s.mutex_acquire_for_test(0));
        assert!(s.is_blocked_on_mutex_for_test(2, 0));
        // El dueño (idx0) hereda la prioridad del waiter más alto (Kernel=0).
        assert_eq!(s.task_priority(0), Priority::Kernel as u8);

        // idx0 libera: la propiedad pasa a idx2 y la prioridad del 0 se restaura.
        s.set_current_for_test(0);
        assert_eq!(s.mutex_unlock(0), 0);
        assert_eq!(s.mutex_owner_for_test(0), Some(2));
        assert_eq!(s.task_priority(0), Priority::App as u8);
        assert!(s.task_alive(2)); // idx2 vuelve a estar Ready
    }

    #[test]
    fn mutex_unlock_by_non_owner_is_denied() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 1
        s.force_start_for_test();

        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0));
        // idx1 no es dueño → Edenied.
        s.set_current_for_test(1);
        assert_eq!(s.mutex_unlock(0), rugus_core::Errno::Edenied as i32);
    }

    #[test]
    fn semaphore_counting_try_wait_and_post() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.force_start_for_test();
        s.set_current_for_test(0);

        s.sem_init(0, 2);
        assert!(s.sem_try_wait(0)); // 2 → 1
        assert!(s.sem_try_wait(0)); // 1 → 0
        assert!(!s.sem_try_wait(0)); // 0 → agotado
        assert_eq!(s.sem_post(0), 0); // 0 → 1
        assert!(s.sem_try_wait(0)); // 1 → 0
    }
}

#[cfg(test)]
mod mpu_sandbox_tests {
    use super::*;
    use rugus_core::sched::{Priority, Scheduler};

    type Sched = Scheduler<MockArch>;

    #[test]
    fn user_task_exposes_its_mpu_region() {
        let mut s = Sched::new();
        let stack = aligned_stack(1024);
        let base = stack.as_ptr() as u32;
        s.spawn_user(stack, dummy_entry, Priority::App).unwrap();
        s.force_start_for_test();
        // La región App-RW del sandbox es exactamente [base, base+len).
        assert_eq!(s.current_user_region(), Some((base, 1024)));
    }

    #[test]
    fn privileged_task_has_no_user_region() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        // El kernel se confía a sí mismo: sin región userland que validar.
        assert_eq!(s.current_user_region(), None);
    }

    #[test]
    fn aligned_pow2_user_stack_is_accepted() {
        let mut s = Sched::new();
        // 2 KiB potencia de 2 y alineada → cumple el invariante MPU.
        assert!(s
            .spawn_user(aligned_stack(2048), dummy_entry, Priority::App)
            .is_ok());
    }
}

#[cfg(test)]
mod abi_tests {
    use rugus_core::sched::TaskId;
    use rugus_core::syscall::{dispatch, validate_user_range, Hooks, Id, ABI_VERSION};
    use rugus_core::Domain;

    fn h_yield() {}
    fn h_sleep(_ms: u32) {}
    fn h_task_id() -> TaskId {
        TaskId(7)
    }
    fn h_domain() -> Domain {
        Domain::App
    }
    fn region_user() -> Option<(u32, u32)> {
        Some((0x2000_0000, 0x1000))
    }
    fn region_priv() -> Option<(u32, u32)> {
        None
    }
    fn h_ipc(_chan: u32, _msg: u32) -> i32 {
        42
    }
    fn h_sync(_id: u32) -> i32 {
        0
    }

    #[test]
    fn abi_version_is_v1() {
        assert_eq!(ABI_VERSION, 0x0001);
    }

    #[test]
    fn id_from_raw_roundtrips_known_and_rejects_unknown() {
        for raw in [
            0x00u8, 0x01, 0x02, 0x03, 0x10, 0x11, 0x20, 0x21, 0x22, 0x23, 0x30, 0x40, 0xFE, 0xFF,
        ] {
            let id = Id::from_raw(raw).expect("id conocido");
            assert_eq!(id as u8, raw);
        }
        assert!(Id::from_raw(0x99).is_none());
        assert!(Id::from_raw(0x04).is_none());
    }

    // Toca el `HOOKS` global de rugus-core; debe ser el ÚNICO test que lo use
    // para no competir con otros hilos del harness.
    #[test]
    fn dispatch_and_pointer_validation() {
        let hooks = Hooks {
            yield_now: h_yield,
            sleep_ms: h_sleep,
            current_task_id: h_task_id,
            current_domain: h_domain,
            current_user_region: region_user,
            ipc_send: h_ipc,
            mutex_lock: h_sync,
            mutex_unlock: h_sync,
            sem_wait: h_sync,
            sem_post: h_sync,
        };
        // SAFETY: test single-uso del estado global; sin concurrencia con otros.
        unsafe {
            rugus_core::syscall::register(hooks);
        }

        // --- validate_user_range con región userland [0x2000_0000, +0x1000) ---
        assert!(validate_user_range(0x2000_0000, 0x100).is_ok());
        assert!(validate_user_range(0x2000_0000, 0).is_ok()); // len 0 trivial
                                                              // Rango que rebasa el final de la región → rechazo (cierra TOCTOU).
        assert!(validate_user_range(0x2000_0FF0, 0x20).is_err());
        // Puntero por debajo de la base → rechazo.
        assert!(validate_user_range(0x1FFF_FFF0, 0x10).is_err());
        // Overflow de ptr+len → rechazo (checked_add).
        assert!(validate_user_range(0xFFFF_FFF0, 0x20).is_err());

        // --- dispatch de syscalls ---
        assert_eq!(dispatch(Id::TaskId, [0; 4]), 7);
        assert_eq!(dispatch(Id::YieldNow, [0; 4]), 0);
        assert_eq!(dispatch(Id::SleepMs, [10, 0, 0, 0]), 0);
        assert_eq!(dispatch(Id::IpcSend, [1, 2, 0, 0]), 42);
        // Syscalls de sincronización: rutean al hook (id en args[0]) → 0.
        assert_eq!(dispatch(Id::MutexLock, [0, 0, 0, 0]), 0);
        assert_eq!(dispatch(Id::MutexUnlock, [0, 0, 0, 0]), 0);
        assert_eq!(dispatch(Id::SemWait, [0, 0, 0, 0]), 0);
        assert_eq!(dispatch(Id::SemPost, [0, 0, 0, 0]), 0);
        // Syscalls aún no implementadas devuelven Errno negativo.
        assert!(dispatch(Id::Log, [0; 4]) < 0);
        assert!(dispatch(Id::NetSocket, [0; 4]) < 0);

        // --- llamante privilegiado: confiado (sin región) ---
        let priv_hooks = Hooks {
            current_user_region: region_priv,
            ..hooks
        };
        // SAFETY: idem; re-registro secuencial dentro del mismo test.
        unsafe {
            rugus_core::syscall::register(priv_hooks);
        }
        // Cualquier rango es válido para un llamante privilegiado.
        assert!(validate_user_range(0x9999_9999, 0x40).is_ok());
    }
}

#[cfg(test)]
mod fault_tests {
    use rugus_core::fault::{FaultKind, FaultReport};
    use rugus_core::sched::TaskId;
    use rugus_core::Domain;

    #[test]
    fn fault_and_domain_names() {
        assert_eq!(FaultKind::MemManage.name(), "MemManage");
        assert_eq!(FaultKind::HardFault.name(), "HardFault");
        assert_eq!(FaultKind::BusFault.name(), "BusFault");
        assert_eq!(FaultKind::UsageFault.name(), "UsageFault");
        assert_eq!(Domain::Kernel.name(), "Kernel");
        assert_eq!(Domain::App.name(), "App");
        assert_eq!(Domain::Drivers.name(), "Drivers");
        assert_eq!(Domain::Services.name(), "Services");
    }

    #[test]
    fn fault_report_is_copy_and_comparable() {
        let r = FaultReport {
            kind: FaultKind::MemManage,
            pc: 0x0800_1854,
            domain: Domain::App,
            task_id: TaskId(1),
            addr: Some(0x4000_0000),
        };
        let copy = r; // Copy
        assert_eq!(r, copy);
        assert_eq!(copy.addr, Some(0x4000_0000));
    }
}
