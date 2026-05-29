//! Protocolo `IDENTIFY` — descubrimiento de dispositivos Rugus.
//!
//! Un host (rugus-cli de escritorio) envía la línea `IDENTIFY\r\n` o el byte de
//! control `ENQ` (0x05) por cualquier transporte (serie/BLE). El dispositivo
//! responde **exactamente una línea** con su firma:
//!
//! ```text
//! RUGUS;tier=<tier>;chip=<chip>;proto=1;shell=rush;cli=1.0.0\r\n
//! ```
//!
//! Es una respuesta barata: sin lógica pesada, sin asignación dinámica. El
//! kernel sigue siendo serio; el "wow" vive en el host.

use crate::ansi::Write;

/// Byte de control ENQ (Enquiry) que dispara la respuesta IDENTIFY.
pub const ENQ: u8 = 0x05;

/// Versión del protocolo IDENTIFY.
pub const PROTO_VERSION: u8 = 1;

/// Nombre de la shell embebida (campo `shell=`).
pub const SHELL_NAME: &str = "rush";

/// Prefijo obligatorio de toda firma Rugus.
pub const SIGNATURE_PREFIX: &str = "RUGUS;";

/// Tier por defecto del build actual (campo `tier=`). El tier lite es el único
/// objetivo de `rush` hoy; placas futuras pueden sobrescribirlo al llamar a
/// [`write_signature`].
pub const TIER: &str = "lite";

/// Familia de chip por defecto del build actual (campo `chip=`).
pub const CHIP: &str = "f103";

/// Escribe la línea de firma IDENTIFY en `out`.
///
/// `tier` y `chip` son específicos de la placa (p. ej. `"lite"` / `"f103"`).
/// `proto`, `shell` y `cli` los aporta `rush`.
pub fn write_signature(out: &mut dyn Write, tier: &str, chip: &str) {
    let _ = out.write_str(SIGNATURE_PREFIX);
    let _ = out.write_str("tier=");
    let _ = out.write_str(tier);
    let _ = out.write_str(";chip=");
    let _ = out.write_str(chip);
    let _ = out.write_str(";proto=1;shell=");
    let _ = out.write_str(SHELL_NAME);
    let _ = out.write_str(";cli=");
    let _ = out.write_str(crate::CLI_VERSION);
    let _ = out.write_str("\r\n");
}
