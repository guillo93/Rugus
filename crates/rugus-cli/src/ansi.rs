//! Secuencias ANSI — solo capa CLI (cosmética).

/// Reset de atributos.
pub const RESET: &str = "\x1b[0m";
/// Cian brillante.
pub const BRIGHT_CYAN: &str = "\x1b[96m";
/// Magenta brillante.
pub const BRIGHT_MAGENTA: &str = "\x1b[95m";

/// Banner decorativo para `cosmos`.
pub fn cosmos_banner(out: &mut dyn Write) {
    let _ = out.write_str(BRIGHT_CYAN);
    let _ = out.write_str("== cosmos ==\r\n");
    let _ = out.write_str(RESET);
}

/// Banner decorativo para `orbit`.
pub fn orbit_banner(out: &mut dyn Write) {
    let _ = out.write_str(BRIGHT_MAGENTA);
    let _ = out.write_str("== orbit ==\r\n");
    let _ = out.write_str(RESET);
}

/// Trait mínimo de escritura.
pub trait Write {
    /// Escribe una cadena UTF-8.
    fn write_str(&mut self, s: &str) -> Result<(), ()>;
}
