//! Rugus runtime para targets ARM Cortex-M.
//!
//! Provee:
//! - Re-export del macro `#[entry]` de `cortex-m-rt`.
//! - Panic handler vía `panic-probe`.
//! - Transporte de logs `defmt` vía RTT (SWD).
//! - Timestamp `defmt` desde el cycle counter (DWT).
//!
//! Un ejemplo o un firmware solo necesita `use rugus_runtime as _;` para
//! obtener un entorno bare-metal usable con logs.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub use cortex_m_rt::entry;

#[allow(unused_imports)]
use defmt_rtt as _;
#[allow(unused_imports)]
use panic_probe as _;

// `defmt::timestamp!` registra una expresión que produce el timestamp.
// CYCCNT debe estar habilitado por el firmware (ver `enable_cycle_counter`)
// antes del primer log si te importan timestamps correctos.
defmt::timestamp!("{=u32}", cortex_m::peripheral::DWT::cycle_count());

/// Lock Access Register del DWT (`DWT_LAR`, 0xE0001FB0). Escribir la clave
/// `0xC5ACCE55` desbloquea la escritura de los registros del DWT.
const DWT_LAR: *mut u32 = 0xE000_1FB0 as *mut u32;
/// Clave de desbloqueo del CoreSight Software Lock.
const CORESIGHT_UNLOCK: u32 = 0xC5AC_CE55;

/// Habilita el cycle counter (DWT.CYCCNT) usado para timestamps de `defmt`.
///
/// Debe llamarse una sola vez al arranque, antes del primer `defmt::info!`
/// si te importan timestamps correctos.
///
/// En el Cortex-M7 (STM32F769) el DWT arranca con el *software lock* activo, así
/// que `CYCCNTENA` se ignora y el contador queda en cero (timestamps congelados).
/// Desbloqueamos el `DWT_LAR` antes de habilitarlo; en cores sin lock (M4 del
/// F407, M3 del F103) la escritura es inocua.
pub fn enable_cycle_counter(cp: &mut cortex_m::Peripherals) {
    cp.DCB.enable_trace();
    // SAFETY: registro CoreSight estándar en todos los ARMv7-M; escritura única
    // de la clave de desbloqueo en el arranque.
    unsafe {
        core::ptr::write_volatile(DWT_LAR, CORESIGHT_UNLOCK);
    }
    cp.DWT.set_cycle_count(0);
    cp.DWT.enable_cycle_counter();
}
