//! Feedback coloreado directo al sink de la consola.
//!
//! Los verbos informativos (`cosmos`/`coil`/…) componen su salida en un buffer
//! con [`rugus_ui::Painter`]; en cambio el feedback corto de `rush` (auth, OK /
//! error de cada verbo) se escribe directo al [`crate::Write`]. Estos helpers
//! emiten las secuencias ANSI de [`rugus_ui`] respetando el flag global de
//! color: en modo plano no sale ningún byte de escape (los glifos UTF-8 sí).

use crate::Write;
use rugus_ui::{color, Role, RESET};

/// Escribe `code` solo si el color está activo.
fn esc(out: &mut dyn Write, code: &str) {
    if color() {
        let _ = out.write_str(code);
    }
}

/// Escribe `msg` con el color de `role` (sin salto de línea).
pub fn tint(out: &mut dyn Write, role: Role, msg: &str) {
    esc(out, role.code());
    let _ = out.write_str(msg);
    esc(out, RESET);
}

/// Línea de éxito: `✓ <msg>` en verde, con `\r\n`.
pub fn ok(out: &mut dyn Write, msg: &str) {
    esc(out, Role::Core.code());
    let _ = out.write_str("\u{2713} ");
    let _ = out.write_str(msg);
    esc(out, RESET);
    let _ = out.write_str("\r\n");
}

/// Línea de error: `✗ <msg>` en rojo, con `\r\n`.
pub fn err(out: &mut dyn Write, msg: &str) {
    esc(out, Role::Fault.code());
    let _ = out.write_str("\u{2717} ");
    let _ = out.write_str(msg);
    esc(out, RESET);
    let _ = out.write_str("\r\n");
}

/// Prompt de la consola: `rugus:<placa> ▸ ` — `rugus` en verde (núcleo), el
/// separador en gris, la placa en oro (foco) y el cursor `▸` en verde. Sin
/// salto de línea: el cursor queda tras él. Da a `rush` su identidad de shell,
/// distinta de cualquier `$`/`#` de otros sistemas.
pub fn prompt(out: &mut dyn Write, board: &str) {
    esc(out, Role::Core.code());
    let _ = out.write_str("rugus");
    esc(out, RESET);
    tint(out, Role::Chrome, ":");
    tint(out, Role::Focus, board);
    let _ = out.write_str(" ");
    esc(out, Role::Core.code());
    let _ = out.write_str("\u{25b8}");
    esc(out, RESET);
    let _ = out.write_str(" ");
}

/// Línea de aviso: `• <msg>` en ámbar, con `\r\n`.
pub fn warn(out: &mut dyn Write, msg: &str) {
    esc(out, Role::Warn.code());
    let _ = out.write_str("\u{2022} ");
    let _ = out.write_str(msg);
    esc(out, RESET);
    let _ = out.write_str("\r\n");
}
