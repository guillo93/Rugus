//! Base de tiempo y **quantum de preempción** del backend AArch64.
//!
//! Contraparte del `time.rs` de Cortex-M (SysTick): aquí el latido lo da el
//! **Generic Timer** físico (`CNTP_*_EL0`) enrutado al core 0 por los
//! periféricos locales de ARM del BCM2837. En cada vencimiento del quantum, la
//! ISR del IRQ (en el arch crate, [`crate::vectors`]) llama a
//! [`fire_preempt_hook`], que la capa de kernel cablea a
//! `Scheduler::preempt_tick` — exactamente el mismo contrato que en Cortex-M, de
//! modo que el scheduler arch-agnóstico de `rugus-core` se preempta igual en
//! ambas arquitecturas.
//!
//! El reloj monotónico ([`crate::CortexA::now_ms`]) sigue leyendo `CNTPCT_EL0`
//! (libre, sin IRQ); este módulo solo gobierna el temporizador de rodaja.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Routing de interrupciones del timer del core 0 (periféricos locales ARM,
/// base `0x4000_0000`). `CNTPNSIRQ` (bit 1) enruta el IRQ del timer físico
/// no-seguro EL1 (`CNTP`) al core 0.
const CORE0_TIMER_IRQCNTL: usize = 0x4000_0040;
/// Fuente de IRQ pendiente del core 0 (para discriminar el origen en la ISR).
const CORE0_IRQ_SOURCE: usize = 0x4000_0060;
/// Bit del IRQ del timer físico no-seguro EL1 (`CNTPNSIRQ`).
const CNTPNSIRQ: u32 = 1 << 1;

/// Cuentas del Generic Timer por rodaja (quantum), fijadas en [`init`].
static QUANTUM: AtomicU64 = AtomicU64::new(0);
/// Nº de vencimientos del quantum desde [`init`] (telemetría/diagnóstico).
static TIMER_IRQS: AtomicU32 = AtomicU32::new(0);
/// Hook de preempción que la ISR invoca en cada vencimiento. `usize` porque no
/// hay atómico portable de puntero a `fn`; el cast es de ida y vuelta exacto.
static PREEMPT_HOOK: AtomicUsize = AtomicUsize::new(0);
/// `true` cuando [`init`] ha armado la preempción: solo entonces la primera
/// tarea debe arrancar con IRQs habilitadas (ver `CortexA::start_first`).
static ARMED: AtomicBool = AtomicBool::new(false);

#[inline]
fn mmio_read(addr: usize) -> u32 {
    // SAFETY: registro MMIO de 32 bits de los periféricos locales del core.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
fn mmio_write(addr: usize, val: u32) {
    // SAFETY: registro MMIO de 32 bits de los periféricos locales del core.
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

/// Reprograma el Generic Timer para el próximo vencimiento de rodaja y lo
/// habilita. Idempotente: la ISR la llama tras cada vencimiento.
#[inline]
fn rearm() {
    let q = QUANTUM.load(Ordering::Relaxed);
    // SAFETY: `CNTP_TVAL_EL0` = cuenta-atrás hasta el IRQ; `CNTP_CTL_EL0`=1
    // habilita el timer con la máscara de IRQ desactivada (bit 1 = IMASK = 0).
    unsafe {
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) q);
        core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64);
    }
}

/// Programa el Generic Timer como temporizador de rodaja con un periodo de
/// `slice_ms` milisegundos y enruta su IRQ al core 0.
///
/// No habilita las IRQs del core (`DAIF`): eso lo hace el arranque del scheduler
/// (`start_first` con la rodaja en marcha). Tras esto, cada `slice_ms` el timer
/// dispara el IRQ que llama a [`fire_preempt_hook`].
pub fn init(slice_ms: u32) {
    let freq: u64;
    // SAFETY: lectura de la frecuencia del Generic Timer (solo lectura).
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
    }
    let slice = slice_ms.max(1) as u64;
    QUANTUM.store((freq * slice / 1000).max(1), Ordering::Relaxed);
    rearm();
    mmio_write(CORE0_TIMER_IRQCNTL, CNTPNSIRQ);
    ARMED.store(true, Ordering::Relaxed);
}

/// `true` si la preempción está armada (timer inicializado). La consulta
/// `CortexA::start_first` para decidir si habilita IRQs al entrar en la primera
/// tarea; un despliegue puramente cooperativo (sin `init`) la deja en `false` y
/// arranca con IRQs enmascaradas, sin cambio de comportamiento.
#[inline]
pub fn preemption_armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Registra la función de preempción que la ISR del timer llamará en cada
/// vencimiento del quantum.
///
/// La capa de kernel pasa aquí un trampolín que rutea a su scheduler
/// (`preempt_tick`). Sin hook registrado, el timer solo lleva la cuenta de IRQs.
pub fn set_preempt_hook(hook: fn()) {
    PREEMPT_HOOK.store(hook as usize, Ordering::Relaxed);
}

/// Dispara el hook de preempción si hay alguno registrado.
#[inline]
fn fire_preempt_hook() {
    let hook = PREEMPT_HOOK.load(Ordering::Relaxed);
    if hook != 0 {
        // SAFETY: solo se escribe en `set_preempt_hook` con un `fn()` válido; el
        // cast usize→fn() revierte exactamente el store.
        let f: fn() = unsafe { core::mem::transmute(hook) };
        f();
    }
}

/// Atiende un IRQ del core: si es el vencimiento del timer de rodaja, lo
/// reprograma, contabiliza y dispara la preempción. Devuelve `true` si el IRQ
/// era del timer (consumido). La invoca la ISR de [`crate::vectors`].
///
/// El cambio de contexto real ocurre **dentro** de [`fire_preempt_hook`]
/// (→ `preempt_tick` → `switch_context`), anidado en el frame de excepción que
/// la ISR ya apiló: al reanudarse la tarea, la ISR completa su `eret`.
#[inline]
pub fn on_irq() -> bool {
    if mmio_read(CORE0_IRQ_SOURCE) & CNTPNSIRQ == 0 {
        return false;
    }
    rearm();
    TIMER_IRQS.fetch_add(1, Ordering::Relaxed);
    fire_preempt_hook();
    true
}

/// Nº de vencimientos del quantum desde [`init`].
#[inline]
pub fn timer_irqs() -> u32 {
    TIMER_IRQS.load(Ordering::Relaxed)
}
