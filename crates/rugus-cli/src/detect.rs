//! Auto-detección: combina sondeo serie y BLE, devuelve solo dispositivos Rugus.

use crate::device::Candidate;
use crate::{ble, net, serial};

/// Opciones de descubrimiento.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Sondear puertos serie.
    pub serial: bool,
    /// Escanear BLE.
    pub ble: bool,
    /// Descubrir por red (broadcast UDP IDENTIFY).
    pub net: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            serial: true,
            ble: true,
            net: true,
        }
    }
}

/// Ejecuta la detección según `opts` y devuelve los candidatos Rugus válidos.
///
/// Solo se listan dispositivos que respondieron una firma `RUGUS;...` válida;
/// el resto (otros seriales, otros periféricos BLE) se descartan.
pub fn discover(opts: Options) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    if opts.serial {
        candidates.extend(serial::detect());
    }
    if opts.ble {
        candidates.extend(ble::detect());
    }
    if opts.net {
        candidates.extend(net::detect());
    }
    candidates
}
