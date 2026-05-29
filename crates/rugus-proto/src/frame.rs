//! Ensamblado de líneas desde un transporte por bytes (serie/BLE).
//!
//! Los transportes entregan bytes en bloques arbitrarios. [`LineAssembler`]
//! acumula bytes y emite líneas completas terminadas en `\n` (tolerando `\r\n`).
//! Mantiene un límite de longitud para no crecer sin control ante ruido.

/// Límite por defecto de longitud de línea (bytes) antes de truncar.
pub const DEFAULT_MAX_LINE: usize = 512;

/// Acumulador de bytes → líneas de texto.
#[derive(Debug)]
pub struct LineAssembler {
    buf: Vec<u8>,
    max_line: usize,
}

impl Default for LineAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl LineAssembler {
    /// Crea un ensamblador con el límite por defecto.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_LINE)
    }

    /// Crea un ensamblador con un límite de línea explícito.
    pub fn with_capacity(max_line: usize) -> Self {
        Self {
            buf: Vec::with_capacity(max_line.min(DEFAULT_MAX_LINE)),
            max_line: max_line.max(1),
        }
    }

    /// Inserta bytes recibidos y devuelve todas las líneas ya completas.
    ///
    /// Las líneas se devuelven sin el `\r\n`/`\n` final.
    pub fn push(&mut self, data: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in data {
            if b == b'\n' {
                let line = self.take_line();
                out.push(line);
            } else if self.buf.len() < self.max_line {
                self.buf.push(b);
            } else {
                // Línea sobredimensionada: cortar aquí para acotar memoria.
                let line = self.take_line();
                out.push(line);
                self.buf.push(b);
            }
        }
        out
    }

    fn take_line(&mut self) -> String {
        // Quitar `\r` final si lo hay (CRLF).
        if self.buf.last() == Some(&b'\r') {
            self.buf.pop();
        }
        let line = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf.clear();
        line
    }

    /// Devuelve y limpia cualquier resto pendiente (sin terminador).
    pub fn flush(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            None
        } else {
            Some(self.take_line())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_crlf_lines() {
        let mut a = LineAssembler::new();
        let lines = a.push(b"cosmos\r\norbit\r\n");
        assert_eq!(lines, vec!["cosmos".to_string(), "orbit".to_string()]);
    }

    #[test]
    fn handles_partial_chunks() {
        let mut a = LineAssembler::new();
        assert!(a.push(b"RUGUS;tier=").is_empty());
        assert!(a.push(b"lite").is_empty());
        let lines = a.push(b";chip=f103\n");
        assert_eq!(lines, vec!["RUGUS;tier=lite;chip=f103".to_string()]);
    }

    #[test]
    fn bare_lf_works() {
        let mut a = LineAssembler::new();
        assert_eq!(a.push(b"a\nb\n"), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn flush_returns_remainder() {
        let mut a = LineAssembler::new();
        assert!(a.push(b"partial").is_empty());
        assert_eq!(a.flush(), Some("partial".to_string()));
        assert_eq!(a.flush(), None);
    }

    #[test]
    fn caps_oversized_line() {
        let mut a = LineAssembler::with_capacity(4);
        let lines = a.push(b"abcdef\n");
        // Se trunca en 4 y arranca una nueva línea con el resto.
        assert_eq!(lines[0], "abcd");
    }
}
