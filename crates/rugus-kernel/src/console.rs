//! Transporte de recepción para consolas de operador sobre puerto serie.
//!
//! Históricamente este módulo albergaba la consola bespoke de F4.5 (léxico
//! `help`/`ps`/`mem`/`faults`/`respawn`/`reboot`). Con la convergencia F6.4 las
//! consolas de toda la flota hablan el léxico universal `rush` (gateado por
//! autenticación de canal), y el parser/los comandos viven en el crate `rush` +
//! la personalidad de cada placa. Aquí queda solo la pieza de transporte que el
//! kernel sí posee: el anillo SPSC entre la IRQ de RX y la tarea supervisora.
//! Los equivalentes del léxico retirado: `coil` (ps), `cosmos` (mem/uptime),
//! `scar` (faults) y `hatch` (respawn).

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

/// Capacidad del anillo de recepción (potencia de 2 para enmascarar el índice).
const RX_RING_CAP: usize = 128;

/// Anillo SPSC sin bloqueo entre el handler de IRQ de RX (único productor) y la
/// tarea supervisora (único consumidor).
///
/// El productor escribe en `head` y el consumidor lee en `tail`; cada lado
/// publica su avance con `Release` y observa el del otro con `Acquire`. Si el
/// anillo está lleno se descarta el byte más reciente (un overrun de consola no
/// debe corromper la cola). Pensado para vivir en un `static` y ser compartido
/// entre el `#[interrupt]` y el bucle del supervisor.
pub struct RxRing {
    buf: [AtomicU8; RX_RING_CAP],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl Default for RxRing {
    fn default() -> Self {
        Self::new()
    }
}

impl RxRing {
    /// Crea un anillo vacío (usable en contexto `const` para un `static`).
    pub const fn new() -> Self {
        Self {
            buf: [const { AtomicU8::new(0) }; RX_RING_CAP],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Encola un byte desde el productor (ISR). Devuelve `false` si estaba lleno
    /// (el byte se descarta sin corromper la cola).
    pub fn push(&self, b: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % RX_RING_CAP;
        if next == self.tail.load(Ordering::Acquire) {
            return false; // lleno
        }
        self.buf[head].store(b, Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
        true
    }

    /// Saca un byte desde el consumidor (supervisor). `None` si está vacío.
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None; // vacío
        }
        let b = self.buf[tail].load(Ordering::Relaxed);
        self.tail.store((tail + 1) % RX_RING_CAP, Ordering::Release);
        Some(b)
    }
}
