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

#[defmt::timestamp]
fn timestamp() -> u64 {
    // SAFETY: lectura 32-bit alineada de CYCCNT, atómica intrínsecamente
    // en Cortex-M. CYCCNT debe estar habilitado por el firmware antes del
    // primer log; ver [`enable_cycle_counter`].
    let cyccnt = cortex_m::peripheral::DWT::cycle_count();
    cyccnt as u64
}

/// Habilita el cycle counter (DWT.CYCCNT) usado para timestamps de `defmt`.
///
/// Debe llamarse una sola vez al arranque, antes del primer `defmt::info!`
/// si te importan timestamps correctos.
pub fn enable_cycle_counter(cp: &mut cortex_m::Peripherals) {
    cp.DCB.enable_trace();
    cp.DWT.enable_cycle_counter();
}
