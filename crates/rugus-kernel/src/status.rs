//! Visualización del estado del kernel por LEDs, reutilizable por cualquier placa.
//!
//! Antes, cada `main` reimplementaba a mano los patrones de los LEDs de estado
//! (latido, salud, fault) y los conducía con su propio código inline. Esa lógica
//! es idéntica entre placas y depende SOLO del reloj monotónico y del estado del
//! scheduler (que esta capa ya conoce), no de un periférico concreto. Aquí vive
//! una sola vez:
//!
//! - Los **patrones** ([`heartbeat`], [`degraded_blink`]) son funciones puras de
//!   `now_ms` — sin estado, sin hardware.
//! - El **fault latch** lo enciende el propio [`crate::install`]/`fault_hook` al
//!   primer fault contenido y queda pegado; [`refresh`] lo refleja en el LED de
//!   fault sin que la placa registre un observer solo para eso.
//! - La placa implementa [`StatusLeds`] (tres setters infalibles) sobre sus pines
//!   y llama a [`refresh`] en cada muestreo del supervisor; el mapeo rol→pin y el
//!   tragado de errores de GPIO son lo único específico de placa.
//!
//! Frontera de capas: este módulo NO depende de `rugus-hal` ni de ningún
//! periférico; el `trait` invierte la dependencia para que el kernel siga siendo
//! agnóstico al hardware (igual que [`crate::FaultObserver`]).

use core::sync::atomic::{AtomicBool, Ordering};

/// `true` desde el primer fault contenido; nunca se limpia (latch).
static FAULT_LATCH: AtomicBool = AtomicBool::new(false);

/// Marca el latch de fault. La invoca el `fault_hook` del kernel al contener un
/// fault; idempotente.
pub(crate) fn latch_fault() {
    FAULT_LATCH.store(true, Ordering::Relaxed);
}

/// `true` si ya se contuvo al menos un fault desde el arranque.
pub fn fault_latched() -> bool {
    FAULT_LATCH.load(Ordering::Relaxed)
}

/// Pines de estado que la placa expone al servicio de visualización.
///
/// Tres roles derivados del estado del kernel; el LED de actividad de userland
/// NO está aquí porque es semántica de la app (la conduce su propio protocolo
/// IPC), no estado del kernel. Los setters son infalibles: la placa traga el
/// error de su `GpioPin` (un LED que no enciende no debe abortar el supervisor).
pub trait StatusLeds {
    /// Latido del kernel: lo enciende/apaga [`heartbeat`].
    fn set_alive(&mut self, on: bool);
    /// Salud del supervisor: fijo si sano, parpadeo lento si degradado.
    fn set_health(&mut self, on: bool);
    /// Fault contenido: encendido fijo una vez latcheado.
    fn set_fault(&mut self, on: bool);
}

/// Latido "lub-dub": doble pulso corto al inicio de cada ventana de 1 s.
#[inline]
pub fn heartbeat(now_ms: u32) -> bool {
    let t = now_ms % 1000;
    t < 80 || (200..280).contains(&t)
}

/// Parpadeo lento ~1 Hz para señalar estado degradado.
#[inline]
pub fn degraded_blink(now_ms: u32) -> bool {
    (now_ms / 500) % 2 == 0
}

/// Pinta los tres LEDs de estado a partir del reloj y del estado del scheduler.
///
/// La llama el supervisor de la placa en cada muestreo (típicamente ~40 ms):
/// - alive: [`heartbeat`].
/// - health: fijo si `killed_count() == 0`, [`degraded_blink`] si alguna tarea
///   murió.
/// - fault: encendido fijo desde que se latchea el primer fault.
pub fn refresh(now_ms: u32, leds: &mut impl StatusLeds) {
    leds.set_alive(heartbeat(now_ms));
    let healthy = crate::killed_count() == 0;
    leds.set_health(if healthy {
        true
    } else {
        degraded_blink(now_ms)
    });
    leds.set_fault(fault_latched());
}
