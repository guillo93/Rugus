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
        // Lógica de guarda de `respawn`: retorna ANTES de tocar memoria.
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        // Índice fuera de rango → false (sin tocar memoria).
        assert!(!s.respawn(99));
        // Tarea viva (no Killed) → false (sin tocar memoria).
        assert!(!s.respawn(0));
    }

    #[test]
    fn respawn_reconstructs_stack_and_resets_task() {
        // Ejerce la RECONSTRUCCIÓN COMPLETA de stack de `respawn` en host. Antes
        // solo era validable en placa (F3.6: 22 ciclos kill→respawn en F769)
        // porque `stack_base` era `u32` y en el host de 64 bits el puntero quedaba
        // truncado, así que repintar el stack era UB. Con `stack_base: usize`
        // (D3) el puntero se preserva completo y, como `plain_stack` filtra un
        // buffer `'static`, el repintado real es seguro y observable aquí.
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 0 (a respawnear)
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 1 (testigo)
        s.force_start_for_test();

        // Ensucia idx0: hereda prioridad (Kernel) vía PI y arma un plazo de
        // liveness, para verificar que respawn lo deja TODO limpio.
        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0));
        s.set_current_for_test(1);
        assert!(!s.mutex_acquire_for_test(0)); // idx1 bloquea, eleva a idx0
        assert_eq!(s.task_priority(0), Priority::Kernel as u8);
        s.set_liveness_period(0, 1_000);
        assert!(s.liveness_deadline_for_test(0).is_some());

        // idx0 muere (lo mata un fault). Lo respawnea el supervisor desde idx1.
        s.set_current_for_test(1);
        s.mark_killed_for_test(0);
        assert!(s.is_killed_for_test(0));

        // Revive: reconstruye el stack (repinta el buffer real), rearma el frame
        // (MockArch lo deja vacío) y resetea estado/prioridad/liveness.
        assert!(s.respawn(0));
        assert!(s.task_alive(0)); // Killed → Ready
        assert_eq!(
            s.task_priority(0),
            Priority::App as u8,
            "respawn arranca sin prioridad heredada de su vida anterior"
        );
        assert_eq!(
            s.liveness_deadline_for_test(0),
            None,
            "respawn desarma el plazo de liveness viejo"
        );
        // Stack repintado por completo: MockArch::init_task_stack no escribe frame,
        // así que todo el buffer queda con el patrón → high-water 0 (prueba de que
        // el `fill` recorrió el buffer real, no un puntero truncado).
        assert_eq!(s.stack_high_water(0), 0);
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
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 0 (baja)
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
    fn priority_inheritance_propagates_transitively() {
        // Cadena de bloqueo de 3 niveles (inversión de prioridad encadenada):
        //   idx2 (Kernel) espera m1 ── que retiene idx1 (Service)
        //   idx1 (Service) espera m0 ── que retiene idx0 (App)
        // La herencia DEBE propagarse transitivamente: el boost de idx2 tiene que
        // llegar hasta idx0 (Kernel), no quedarse en idx1 (herencia de un nivel).
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 0 (baja)
        s.spawn(plain_stack(512), dummy_entry, Priority::Service)
            .unwrap(); // idx 1 (media)
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 2 (alta)
        s.force_start_for_test();

        // idx0 toma m0; idx1 toma m1.
        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0));
        s.set_current_for_test(1);
        assert!(s.mutex_acquire_for_test(1));

        // idx1 intenta m0 (de idx0) → se bloquea; idx0 hereda Service.
        s.set_current_for_test(1);
        assert!(!s.mutex_acquire_for_test(0));
        assert!(s.is_blocked_on_mutex_for_test(1, 0));
        assert_eq!(s.task_priority(0), Priority::Service as u8);

        // idx2 intenta m1 (de idx1) → se bloquea; idx1 hereda Kernel y el boost
        // se PROPAGA por la cadena hasta idx0, que también pasa a Kernel.
        s.set_current_for_test(2);
        assert!(!s.mutex_acquire_for_test(1));
        assert!(s.is_blocked_on_mutex_for_test(2, 1));
        assert_eq!(s.task_priority(1), Priority::Kernel as u8);
        assert_eq!(
            s.task_priority(0),
            Priority::Kernel as u8,
            "el boost de idx2 debe propagarse transitivamente hasta idx0"
        );

        // idx0 libera m0: cede la propiedad a idx1 y suelta su prioridad prestada.
        // idx1 sigue Kernel (aún retiene m1 con idx2 esperando); idx0 vuelve a App.
        s.set_current_for_test(0);
        assert_eq!(s.mutex_unlock(0), 0);
        assert_eq!(s.mutex_owner_for_test(0), Some(1));
        assert_eq!(s.task_priority(0), Priority::App as u8);
        assert_eq!(s.task_priority(1), Priority::Kernel as u8);
    }

    #[test]
    fn deadlock_cycle_is_detected_via_wait_graph() {
        // Deadlock clásico de 2 tareas con 2 mutexes (orden de toma cruzado):
        //   idx0 retiene m0 y pide m1 (de idx1) → se bloquea.
        //   idx1 retiene m1 y pide m0 (de idx0) → cierra el ciclo del grafo de
        //   espera owner→mutex. La detección (F5.D.3) debe anotarlo.
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 0
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 1
        s.force_start_for_test();

        // Sin contención todavía: ningún deadlock.
        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0)); // idx0 ← m0
        s.set_current_for_test(1);
        assert!(s.mutex_acquire_for_test(1)); // idx1 ← m1
        assert_eq!(s.deadlock_count(), 0);

        // idx0 pide m1 (de idx1) → se bloquea, pero aún no hay ciclo (idx1 corre).
        s.set_current_for_test(0);
        assert!(!s.mutex_acquire_for_test(1));
        assert!(s.is_blocked_on_mutex_for_test(0, 1));
        assert_eq!(s.deadlock_count(), 0, "una sola arista no cierra ciclo");

        // idx1 pide m0 (de idx0) → cierra el ciclo: deadlock detectado.
        s.set_current_for_test(1);
        assert!(!s.mutex_acquire_for_test(0));
        assert!(s.is_blocked_on_mutex_for_test(1, 0));
        assert_eq!(s.deadlock_count(), 1, "el ciclo idx0↔idx1 debe detectarse");
        assert_eq!(
            s.last_deadlock(),
            Some((1, 0)),
            "la arista que cerró el ciclo es (idx1, m0)"
        );
    }

    #[test]
    fn linear_blocking_chain_is_not_a_deadlock() {
        // Cadena lineal de 3 niveles SIN ciclo (la misma del test de PI
        // transitiva): nadie espera un mutex que retenga una tarea aguas abajo,
        // así que el detector NO debe marcar deadlock.
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 0
        s.spawn(plain_stack(512), dummy_entry, Priority::Service)
            .unwrap(); // idx 1
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 2
        s.force_start_for_test();

        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0)); // idx0 ← m0
        s.set_current_for_test(1);
        assert!(s.mutex_acquire_for_test(1)); // idx1 ← m1

        // idx1 espera m0 (de idx0); idx2 espera m1 (de idx1). Cadena abierta.
        s.set_current_for_test(1);
        assert!(!s.mutex_acquire_for_test(0));
        s.set_current_for_test(2);
        assert!(!s.mutex_acquire_for_test(1));

        assert_eq!(
            s.deadlock_count(),
            0,
            "una cadena lineal de bloqueo no es un deadlock"
        );
        assert_eq!(s.last_deadlock(), None);
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

    #[test]
    fn condvar_signal_wakes_highest_priority_waiter() {
        // Dos tareas bloqueadas en la misma condvar; signal despierta SOLO a la de
        // mayor prioridad (Kernel < App) y suelta el mutex que liberaron al dormir.
        // Plazo lejano (clock=0) para que el barrido de yield_now no las venza.
        reset_mock();
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 0 (baja)
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 1 (alta)
        s.force_start_for_test();

        // idx0 toma el mutex 0 y se duerme en la condvar 0 (lo libera al dormir).
        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0));
        s.block_cond_for_test(0, 0, 10_000);
        assert!(s.is_blocked_on_cond_for_test(0, 0));
        assert_eq!(s.mutex_owner_for_test(0), None, "el wait soltó el mutex");

        // idx1 también toma el mutex y se duerme en la condvar.
        s.set_current_for_test(1);
        assert!(s.mutex_acquire_for_test(0));
        s.block_cond_for_test(0, 0, 10_000);
        assert_eq!(s.cond_waiters_for_test(0), 2);

        // signal despierta al de mayor prioridad (idx1, Kernel) y deja al otro.
        assert_eq!(s.condvar_signal(0), 0);
        assert!(
            !s.is_blocked_on_cond_for_test(1, 0),
            "idx1 (Kernel) despertó"
        );
        assert!(s.task_alive(1));
        assert!(s.is_blocked_on_cond_for_test(0, 0), "idx0 sigue dormido");
        assert_eq!(s.cond_waiters_for_test(0), 1);
    }

    #[test]
    fn condvar_broadcast_wakes_all_waiters() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::App)
            .unwrap(); // idx 0
        s.spawn(plain_stack(512), dummy_entry, Priority::Service)
            .unwrap(); // idx 1
        s.force_start_for_test();

        for idx in 0..2 {
            s.set_current_for_test(idx);
            assert!(s.mutex_acquire_for_test(0));
            s.block_cond_for_test(0, 0, 10_000);
        }
        assert_eq!(s.cond_waiters_for_test(0), 2);

        assert_eq!(s.condvar_broadcast(0), 0);
        assert_eq!(s.cond_waiters_for_test(0), 0, "broadcast vació la condvar");
        assert!(s.task_alive(0));
        assert!(s.task_alive(1));
    }

    #[test]
    fn condvar_wait_times_out_via_wake_expired() {
        // Un wait con plazo: al rebasar el reloj el deadline, el barrido lo saca de
        // la condvar y lo marca Ready (condvar_wait devolvería Etimedout tras
        // re-adquirir el mutex). Sin señal de por medio.
        reset_mock();
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.force_start_for_test();

        s.set_current_for_test(0);
        assert!(s.mutex_acquire_for_test(0));
        set_clock(50);
        s.block_cond_for_test(0, 0, 100); // plazo absoluto t=100
        assert!(s.is_blocked_on_cond_for_test(0, 0));

        // Antes del plazo: sigue bloqueado.
        set_clock(90);
        s.wake_expired_for_test();
        assert!(s.is_blocked_on_cond_for_test(0, 0));

        // Tras el plazo: el barrido lo despierta y lo quita de la condvar.
        set_clock(100);
        s.wake_expired_for_test();
        assert!(!s.is_blocked_on_cond_for_test(0, 0), "venció el plazo");
        assert_eq!(s.cond_waiters_for_test(0), 0);
        assert!(s.task_alive(0));
    }

    #[test]
    fn condvar_wait_rejects_bad_ids_and_non_owner() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.force_start_for_test();
        s.set_current_for_test(0);

        // Ids fuera de rango → Einval.
        assert_eq!(s.condvar_wait(99, 0, 0), rugus_core::Errno::Einval as i32);
        assert_eq!(s.condvar_wait(0, 99, 0), rugus_core::Errno::Einval as i32);
        // No es dueño del mutex → Edenied.
        assert_eq!(s.condvar_wait(0, 0, 0), rugus_core::Errno::Edenied as i32);
        // Ids inexistentes en signal/broadcast → Einval.
        assert_eq!(s.condvar_signal(99), rugus_core::Errno::Einval as i32);
        assert_eq!(s.condvar_broadcast(99), rugus_core::Errno::Einval as i32);
    }

    // --- Barreras (F5.D.2) ---

    #[test]
    fn barrier_opens_when_threshold_reached() {
        // Barrera de 3: las dos primeras llegadas bloquean; la tercera abre y
        // libera a todas, dejando la barrera reiniciada (waiters=0).
        let mut s = Sched::new();
        for _ in 0..3 {
            s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
                .unwrap();
        }
        s.force_start_for_test();
        s.barrier_init(0, 3);

        s.set_current_for_test(0);
        assert!(!s.barrier_arrive_for_test(0), "1ª llegada bloquea");
        assert!(s.is_blocked_on_barrier_for_test(0, 0));

        s.set_current_for_test(1);
        assert!(!s.barrier_arrive_for_test(0), "2ª llegada bloquea");
        assert!(s.is_blocked_on_barrier_for_test(1, 0));

        s.set_current_for_test(2);
        assert!(s.barrier_arrive_for_test(0), "3ª llegada abre la barrera");
        // Todas reanudadas y barrera reiniciada.
        assert!(s.task_alive(0));
        assert!(s.task_alive(1));
        assert!(s.task_alive(2));
        assert!(!s.is_blocked_on_barrier_for_test(0, 0));

        // Reutilizable: vuelve a bloquear en el siguiente ciclo.
        s.set_current_for_test(0);
        assert!(!s.barrier_arrive_for_test(0));
        assert!(s.is_blocked_on_barrier_for_test(0, 0));
    }

    #[test]
    fn barrier_wait_rejects_unconfigured_and_bad_id() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        s.set_current_for_test(0);
        // Id fuera de rango → Einval.
        assert_eq!(s.barrier_wait(99), rugus_core::Errno::Einval as i32);
        // Barrera sin configurar (threshold 0) → Einval.
        assert_eq!(s.barrier_wait(0), rugus_core::Errno::Einval as i32);
    }

    // --- Grupos de eventos (F5.D.2) ---

    #[test]
    fn event_set_wakes_waiters_by_mask_mode() {
        // idx0 espera CUALQUIERA de {bit0,bit1}; idx1 espera TODOS {bit0,bit1}.
        // event_set(bit0) despierta solo a idx0; event_set(bit1) completa idx1.
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 1
        s.force_start_for_test();

        s.set_current_for_test(0);
        s.block_event_for_test(0, 0b11, false, 10_000); // any
        s.set_current_for_test(1);
        s.block_event_for_test(0, 0b11, true, 10_000); // all

        // Fija bit0: satisface a idx0 (any) pero no a idx1 (all).
        assert_eq!(s.event_set(0, 0b01), 0);
        assert!(!s.is_blocked_on_event_for_test(0, 0), "idx0 (any) despertó");
        assert!(
            s.is_blocked_on_event_for_test(1, 0),
            "idx1 (all) sigue esperando"
        );
        assert_eq!(s.event_get(0), 0b01);

        // Fija bit1: ahora idx1 (all) tiene ambos bits → despierta.
        assert_eq!(s.event_set(0, 0b10), 0);
        assert!(!s.is_blocked_on_event_for_test(1, 0), "idx1 (all) despertó");
        assert_eq!(s.event_get(0), 0b11);
    }

    #[test]
    fn event_wait_non_blocking_and_clear() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        s.set_current_for_test(0);

        // Sin bits y timeout 0 → Ebusy (no bloquea).
        assert_eq!(
            s.event_wait(0, 0b01, false, 0),
            rugus_core::Errno::Ebusy as i32
        );
        // Con el bit fijado, la espera no bloqueante pasa.
        assert_eq!(s.event_set(0, 0b01), 0);
        assert_eq!(s.event_wait(0, 0b01, false, 0), 0);
        // clear retira el bit; vuelve a fallar sin bloquear.
        assert_eq!(s.event_clear(0, 0b01), 0);
        assert_eq!(s.event_get(0), 0);
        assert_eq!(
            s.event_wait(0, 0b01, false, 0),
            rugus_core::Errno::Ebusy as i32
        );
        // Ids fuera de rango.
        assert_eq!(s.event_set(99, 1), rugus_core::Errno::Einval as i32);
        assert_eq!(
            s.event_wait(99, 1, false, 0),
            rugus_core::Errno::Einval as i32
        );
    }

    #[test]
    fn event_wait_times_out_via_wake_expired() {
        reset_mock();
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        s.set_current_for_test(0);

        set_clock(50);
        s.block_event_for_test(0, 0b01, false, 100); // plazo t=100
        assert!(s.is_blocked_on_event_for_test(0, 0));

        set_clock(90);
        s.wake_expired_for_test();
        assert!(
            s.is_blocked_on_event_for_test(0, 0),
            "antes del plazo, sigue"
        );

        set_clock(100);
        s.wake_expired_for_test();
        assert!(!s.is_blocked_on_event_for_test(0, 0), "venció el plazo");
        assert!(s.task_alive(0));
    }
}

#[cfg(test)]
mod ipc_tests {
    //! IPC bloqueante por canal con timeout/deadline (F4.2).
    //!
    //! La API bloqueante (`chan_send`/`chan_recv` con espera) usaría
    //! `switch_until_ready`, que con el `switch_context` no-op del host no
    //! progresaría. Por eso se ejercen: (a) la ruta NO bloqueante (`timeout = 0`),
    //! (b) la contabilidad de despertar vía accesores `*_for_test`.
    use super::*;
    use rugus_core::sched::{Priority, Scheduler, CHAN_CAPACITY};
    use rugus_core::Errno;

    type Sched = Scheduler<MockArch>;

    #[test]
    fn channel_fifo_non_blocking() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        s.set_current_for_test(0);

        let mut got = 0u32;
        // Encola dos; se reciben en orden FIFO.
        assert_eq!(s.chan_send(0, 0x11, 0), 0);
        assert_eq!(s.chan_send(0, 0x22, 0), 0);
        assert_eq!(s.chan_len_for_test(0), 2);
        assert_eq!(s.chan_recv(0, 0, &mut got), 0);
        assert_eq!(got, 0x11);
        assert_eq!(s.chan_recv(0, 0, &mut got), 0);
        assert_eq!(got, 0x22);
        // Vacío y sin bloquear → Ebusy.
        assert_eq!(s.chan_recv(0, 0, &mut got), Errno::Ebusy as i32);
    }

    #[test]
    fn channel_full_non_blocking_returns_ebusy() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        s.force_start_for_test();
        s.set_current_for_test(0);

        for i in 0..CHAN_CAPACITY {
            assert_eq!(s.chan_send(0, i as u32, 0), 0);
        }
        // Buffer lleno y sin bloquear → Ebusy.
        assert_eq!(s.chan_send(0, 0xFF, 0), Errno::Ebusy as i32);
    }

    #[test]
    fn send_wakes_blocked_receiver() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 0
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap(); // idx 1
        s.force_start_for_test();

        // idx0 se bloquea esperando recibir (plazo lejano, no vence).
        s.set_current_for_test(0);
        s.block_recv_for_test(0, 1_000_000);
        assert!(s.is_blocked_on_recv_for_test(0, 0));
        assert!(!s.task_alive(0));

        // idx1 envía: encola el mensaje y despierta a idx0.
        s.set_current_for_test(1);
        assert_eq!(s.chan_send(0, 0xCAFE, 0), 0);
        assert!(!s.is_blocked_on_recv_for_test(0, 0));
        assert!(s.task_alive(0)); // idx0 vuelve a estar Ready
        assert_eq!(s.chan_len_for_test(0), 1); // mensaje en el buffer

        // idx0 reanuda y desencola su mensaje.
        s.set_current_for_test(0);
        let mut got = 0u32;
        assert_eq!(s.chan_recv(0, 0, &mut got), 0);
        assert_eq!(got, 0xCAFE);
    }

    #[test]
    fn blocked_receiver_times_out_on_deadline() {
        let mut s = Sched::new();
        s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
            .unwrap();
        reset_mock();
        s.force_start_for_test();
        s.set_current_for_test(0);

        // Bloqueo con plazo en t=100 ms.
        s.block_recv_for_test(0, 100);
        // Antes del plazo: sigue bloqueado.
        set_clock(50);
        s.wake_expired_for_test();
        assert!(s.is_blocked_on_recv_for_test(0, 0));
        // Al alcanzar el plazo: el barrido lo despierta (lo reanudaría con
        // Etimedout) y lo saca de la lista de waiters.
        set_clock(100);
        s.wake_expired_for_test();
        assert!(!s.is_blocked_on_recv_for_test(0, 0));
        assert!(s.task_alive(0));
    }
}

#[cfg(test)]
mod liveness_tests {
    //! Monitor de liveness / deadline por tarea (F4.3).
    //!
    //! Detecta tareas VIVAS que dejan de progresar (sin crash, así que el fault
    //! containment no las ve). Se ejerce con el reloj controlable (`set_clock`):
    //! armar un periodo, comprobar que no vence antes del plazo, que vence al
    //! rebasarlo, que `checkin` lo renueva y que `force_kill` recupera.
    use super::*;
    use rugus_core::sched::{Priority, Scheduler};

    type Sched = Scheduler<MockArch>;

    /// Crea un scheduler con `n` tareas privilegiadas arrancado, current=0.
    fn sched_with(n: usize) -> Sched {
        let mut s = Sched::new();
        for _ in 0..n {
            s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
                .unwrap();
        }
        reset_mock();
        s.force_start_for_test();
        s.set_current_for_test(0);
        s
    }

    #[test]
    fn arming_sets_deadline_in_the_future() {
        let mut s = sched_with(2);
        set_clock(1000);
        s.set_liveness_period(1, 500);
        // Plazo = ahora + periodo.
        assert_eq!(s.liveness_deadline_for_test(1), Some(1500));
        // Periodo 0 desarma.
        s.set_liveness_period(1, 0);
        assert_eq!(s.liveness_deadline_for_test(1), None);
    }

    #[test]
    fn not_overdue_before_deadline_then_overdue_after() {
        let mut s = sched_with(2);
        set_clock(0);
        s.set_liveness_period(1, 100);
        // current=0; la tarea 1 está monitorizada.
        set_clock(50);
        assert_eq!(s.liveness_overdue(), None);
        set_clock(100);
        // Al alcanzar el plazo, se declara colgada.
        assert_eq!(s.liveness_overdue(), Some(1));
    }

    #[test]
    fn checkin_renews_the_deadline() {
        let mut s = sched_with(2);
        set_clock(0);
        s.set_liveness_period(1, 100);
        set_clock(80);
        // Latido antes de vencer → nuevo plazo = 80 + 100 = 180.
        s.liveness_checkin(1);
        assert_eq!(s.liveness_deadline_for_test(1), Some(180));
        set_clock(100);
        assert_eq!(s.liveness_overdue(), None);
        set_clock(180);
        assert_eq!(s.liveness_overdue(), Some(1));
    }

    #[test]
    fn current_task_is_never_overdue() {
        let mut s = sched_with(2);
        set_clock(0);
        // Monitoriza la tarea 0, que es la actual.
        s.set_liveness_period(0, 100);
        set_clock(1000);
        // La tarea en ejecución progresa por definición → nunca colgada.
        assert_eq!(s.liveness_overdue(), None);
    }

    #[test]
    fn force_kill_recovers_a_hung_task() {
        let mut s = sched_with(2);
        set_clock(0);
        s.set_liveness_period(1, 100);
        set_clock(200);
        let idx = s.liveness_overdue().expect("tarea 1 colgada");
        assert_eq!(idx, 1);
        // No se puede matar a la tarea actual por esta vía.
        assert!(!s.force_kill(0));
        // Se mata la colgada; queda Killed y sin liveness.
        assert!(s.force_kill(1));
        assert!(s.is_killed_for_test(1));
        assert_eq!(s.liveness_deadline_for_test(1), None);
        // Ya no aparece como colgada (las Killed las gestiona respawn).
        assert_eq!(s.liveness_overdue(), None);
        // NOTA: la reconstrucción de stack de `respawn` (y su limpieza del plazo
        // de liveness) no se ejerce en host: el scheduler guarda `stack_base`
        // como u32 (diseño MCU 32-bit) y truncaría un puntero de 64-bit del
        // host. Ese camino se valida en placa (F407+F769).
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
    fn h_chan_send(_chan: u32, _msg: u32, _timeout: u32) -> i32 {
        99
    }
    fn h_chan_recv(_chan: u32, _timeout: u32, _out_ptr: u32) -> i32 {
        0
    }
    fn h_checkin() {}

    #[test]
    fn abi_version_is_v1() {
        assert_eq!(ABI_VERSION, 0x0001);
    }

    #[test]
    fn id_from_raw_roundtrips_known_and_rejects_unknown() {
        for raw in [
            0x00u8, 0x01, 0x02, 0x03, 0x04, 0x10, 0x11, 0x12, 0x13, 0x20, 0x21, 0x22, 0x23, 0x30,
            0x40, 0xFE, 0xFF,
        ] {
            let id = Id::from_raw(raw).expect("id conocido");
            assert_eq!(id as u8, raw);
        }
        assert!(Id::from_raw(0x99).is_none());
        assert!(Id::from_raw(0x05).is_none());
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
            chan_send: h_chan_send,
            chan_recv: h_chan_recv,
            checkin: h_checkin,
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
        // ChanSend: por valor (chan/msg/timeout) → rutea al hook → 99.
        assert_eq!(dispatch(Id::ChanSend, [0, 0xAB, 0, 0]), 99);
        // ChanRecv: valida el out-ptr (args[2]) contra la región userland antes
        // del hook; out_ptr dentro de [0x2000_0000,+0x1000) → ok → hook → 0.
        assert_eq!(dispatch(Id::ChanRecv, [0, 0, 0x2000_0000, 0]), 0);
        // out_ptr fuera de la región del llamante → Efault (sin tocar el hook).
        assert!(dispatch(Id::ChanRecv, [0, 0, 0x1FFF_FFFF, 0]) < 0);
        // Checkin: sin args ni punteros, rutea al hook (no-op) → 0.
        assert_eq!(dispatch(Id::Checkin, [0; 4]), 0);
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

/// Determinismo del scheduler (F4.8): la planificación es una función pura del
/// estado observable (tabla de tareas + reloj explícito). No hay fuentes de
/// no-determinismo (RNG, reloj implícito, orden de hash): la MISMA secuencia de
/// operaciones produce SIEMPRE la MISMA secuencia de tareas, y la rotación
/// round-robin tiene periodo acotado e igual al nº de tareas listas en la banda.
/// Esto sustenta el análisis de latencia: el peor caso de `pick`/`preempt_tick`
/// es un barrido O(MAX_TASKS) sin recursión ni espera no acotada.
#[cfg(test)]
mod determinism_tests {
    use super::*;
    use rugus_core::sched::{Priority, Scheduler};

    type Sched = Scheduler<MockArch>;

    /// Construye un scheduler con `n` tareas kernel y lo arranca de forma
    /// reproducible (reloj reseteado).
    fn fresh(n: usize) -> Sched {
        let mut s = Sched::new();
        for _ in 0..n {
            s.spawn(plain_stack(512), dummy_entry, Priority::Kernel)
                .expect("spawn");
        }
        reset_mock();
        s.force_start_for_test();
        s
    }

    /// Ejecuta `steps` cesiones y devuelve la secuencia de TaskId servidos.
    fn run_yield_sequence(s: &mut Sched, steps: usize) -> Vec<u8> {
        let mut seq = Vec::with_capacity(steps + 1);
        seq.push(s.current_id().0);
        for _ in 0..steps {
            s.yield_now();
            seq.push(s.current_id().0);
        }
        seq
    }

    #[test]
    fn identical_runs_produce_identical_schedule() {
        // Dos schedulers construidos y operados de forma idéntica deben generar
        // EXACTAMENTE la misma traza de planificación: planificación determinista.
        let mut a = fresh(4);
        let mut b = fresh(4);
        let seq_a = run_yield_sequence(&mut a, 64);
        let seq_b = run_yield_sequence(&mut b, 64);
        assert_eq!(seq_a, seq_b);
    }

    #[test]
    fn round_robin_period_equals_ready_count() {
        // Con N tareas listas en una banda, la rotación round-robin es cíclica de
        // periodo exactamente N (sin deriva): tras N cesiones se vuelve al inicio.
        for n in 2..=4usize {
            let mut s = fresh(n);
            let seq = run_yield_sequence(&mut s, n);
            // El primer y el (N+1)-ésimo elemento coinciden (cierre del ciclo)...
            assert_eq!(
                seq.first(),
                seq.last(),
                "el ciclo debe cerrarse tras {n} cesiones"
            );
            // ...y los N primeros son una permutación de todos los TaskId [0, N).
            let mut band: Vec<u8> = seq[..n].to_vec();
            band.sort_unstable();
            let all: Vec<u8> = (0..n as u8).collect();
            assert_eq!(band, all, "round-robin debe servir cada tarea una vez");
        }
    }

    #[test]
    fn preempt_tick_is_deterministic_across_runs() {
        // La preempción por rodaja también es función pura del estado: misma
        // cadencia de ticks → misma traza.
        fn drive(steps: usize) -> Vec<u8> {
            let mut s = fresh(3);
            let mut seq = vec![s.current_id().0];
            for _ in 0..steps {
                s.preempt_tick();
                seq.push(s.current_id().0);
            }
            seq
        }
        assert_eq!(drive(50), drive(50));
    }
}
