//! Heap global sobre región configurable (p. ej. SDRAM externa).
//!
//! Usa `linked_list_allocator::LockedHeap`. El firmware debe llamar a [`init`]
//! una vez con el rango reservado en `memory.x` antes de usar `alloc`.

use linked_list_allocator::LockedHeap;

/// Allocator global del kernel. Inicializar con [`init`] antes de usar.
#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

/// Reserva la región `[start, start + size)` como heap del sistema.
///
/// # Safety
///
/// `start` debe apuntar a `size` bytes contiguos no usados por otra parte
/// del firmware (p. ej. región `SDRAM` del linker script).
pub unsafe fn init(start: *mut u8, size: usize) {
    unsafe {
        HEAP.lock().init(start, size);
    }
}

/// Bytes actualmente asignados en el heap.
pub fn used() -> usize {
    HEAP.lock().used()
}

/// Bytes libres en el heap.
pub fn free() -> usize {
    HEAP.lock().free()
}

/// Tamaño total del heap (usado + libre).
pub fn size() -> usize {
    let h = HEAP.lock();
    h.used() + h.free()
}
