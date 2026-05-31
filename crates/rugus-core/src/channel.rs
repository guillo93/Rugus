//! Canal SPSC (single-producer / single-consumer) acotado y sin bloqueo.
//!
//! Primitiva de flujo entre dos tareas: una produce, otra consume, sin alloc ni
//! secciones críticas. El almacenamiento es un anillo estático de capacidad fija
//! `N`; la sincronización son dos índices atómicos con ordenamiento
//! adquirir/liberar, el mismo patrón que los rings RX de los UART pero genérico
//! sobre `T`.
//!
//! # Contrato
//!
//! - **Un único productor**: solo una tarea/contexto llama [`Channel::try_send`].
//! - **Un único consumidor**: solo una tarea/contexto llama [`Channel::try_recv`].
//!
//! Llamar a `try_send` desde dos contextos a la vez (o `try_recv` desde dos) es
//! *unsound* y rompe las invariantes. El caso típico seguro es ISR-productor /
//! tarea-consumidora, o tarea-a-tarea bajo el scheduler cooperativo.
//!
//! Como en cualquier anillo con índices, se reserva una ranura para distinguir
//! «lleno» de «vacío»: la capacidad útil es `N - 1`.
//!
//! No toca el ABI de syscalls: es una utilidad de `rugus-core` que las capas
//! superiores componen libremente.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Canal SPSC acotado de capacidad `N` (útil `N - 1`).
pub struct Channel<T, const N: usize> {
    buf: [UnsafeCell<MaybeUninit<T>>; N],
    /// Índice de escritura; lo muta solo el productor.
    head: AtomicUsize,
    /// Índice de lectura; lo muta solo el consumidor.
    tail: AtomicUsize,
}

// SAFETY: el acceso a `buf` está particionado por el protocolo SPSC — el
// productor solo escribe la ranura `head` antes de publicar el nuevo `head`
// (Release), y el consumidor solo lee la ranura `tail` tras observar `head`
// (Acquire). Con un único productor y un único consumidor no hay dos accesos
// concurrentes a la misma ranura. `T: Send` basta para mover valores entre los
// dos contextos.
unsafe impl<T: Send, const N: usize> Sync for Channel<T, N> {}

impl<T, const N: usize> Channel<T, N> {
    /// Crea un canal vacío. `N` debe ser ≥ 2 (una ranura se reserva como
    /// centinela lleno/vacío); con `N < 2` el canal nunca acepta elementos.
    pub const fn new() -> Self {
        Self {
            buf: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Capacidad útil (elementos máximos en vuelo).
    pub const fn capacity(&self) -> usize {
        N - 1
    }

    /// Encola `val`. Devuelve `Err(val)` si el canal está lleno (sin perder el
    /// valor, para que el productor decida). Solo el productor debe llamarlo.
    pub fn try_send(&self, val: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % N;
        if next == self.tail.load(Ordering::Acquire) {
            return Err(val); // lleno
        }
        // SAFETY: el productor es el único escritor de la ranura `head`, y el
        // consumidor no la leerá hasta que publiquemos `next` con Release.
        unsafe {
            (*self.buf[head].get()).write(val);
        }
        self.head.store(next, Ordering::Release);
        Ok(())
    }

    /// Saca el siguiente elemento, o `None` si está vacío. Solo el consumidor
    /// debe llamarlo.
    pub fn try_recv(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None; // vacío
        }
        // SAFETY: la ranura `tail` fue inicializada por el productor antes de
        // publicar su `head` (que ya observamos con Acquire); la leemos una sola
        // vez y avanzamos `tail`, así que no hay doble lectura ni aliasing.
        let val = unsafe { (*self.buf[tail].get()).assume_init_read() };
        self.tail.store((tail + 1) % N, Ordering::Release);
        Some(val)
    }

    /// `true` si no hay elementos en vuelo.
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }

    /// `true` si no caben más elementos.
    pub fn is_full(&self) -> bool {
        let next = (self.head.load(Ordering::Relaxed) + 1) % N;
        next == self.tail.load(Ordering::Acquire)
    }

    /// Número aproximado de elementos en vuelo (snapshot relajado).
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail) % N
    }
}

impl<T, const N: usize> Default for Channel<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Drop for Channel<T, N> {
    fn drop(&mut self) {
        // Drena los elementos vivos restantes para ejecutar su Drop. Tras esto
        // las ranuras quedan lógicamente vacías.
        while self.try_recv().is_some() {}
    }
}
