//! Modelo de render — convierte texto con secuencias ANSI SGR en spans.
//!
//! El dispositivo (`rush`) emite secuencias SGR sencillas (`\x1b[0m`, `\x1b[96m`,
//! …). La TUI del host necesita texto + estilo separados del flujo de bytes.
//! [`StyledLine::parse`] produce una lista de [`Span`] que el frontend (ratatui)
//! traduce a sus propios estilos.

/// Color de primer plano (paleta ANSI básica + brillantes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    /// Color por defecto del terminal.
    #[default]
    Default,
    /// Negro.
    Black,
    /// Rojo.
    Red,
    /// Verde.
    Green,
    /// Amarillo.
    Yellow,
    /// Azul.
    Blue,
    /// Magenta.
    Magenta,
    /// Cian.
    Cyan,
    /// Blanco.
    White,
    /// Variante brillante de un color base.
    Bright(BaseColor),
}

/// Colores base usados por las variantes brillantes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseColor {
    /// Negro brillante (gris).
    Black,
    /// Rojo brillante.
    Red,
    /// Verde brillante.
    Green,
    /// Amarillo brillante.
    Yellow,
    /// Azul brillante.
    Blue,
    /// Magenta brillante.
    Magenta,
    /// Cian brillante.
    Cyan,
    /// Blanco brillante.
    White,
}

/// Estilo de un fragmento de texto.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Style {
    /// Color de primer plano.
    pub fg: Color,
    /// Negrita / intensidad alta.
    pub bold: bool,
}

impl Style {
    fn reset(&mut self) {
        *self = Style::default();
    }

    fn apply_sgr(&mut self, code: u32) {
        match code {
            0 => self.reset(),
            1 => self.bold = true,
            22 => self.bold = false,
            30 => self.fg = Color::Black,
            31 => self.fg = Color::Red,
            32 => self.fg = Color::Green,
            33 => self.fg = Color::Yellow,
            34 => self.fg = Color::Blue,
            35 => self.fg = Color::Magenta,
            36 => self.fg = Color::Cyan,
            37 => self.fg = Color::White,
            39 => self.fg = Color::Default,
            90 => self.fg = Color::Bright(BaseColor::Black),
            91 => self.fg = Color::Bright(BaseColor::Red),
            92 => self.fg = Color::Bright(BaseColor::Green),
            93 => self.fg = Color::Bright(BaseColor::Yellow),
            94 => self.fg = Color::Bright(BaseColor::Blue),
            95 => self.fg = Color::Bright(BaseColor::Magenta),
            96 => self.fg = Color::Bright(BaseColor::Cyan),
            97 => self.fg = Color::Bright(BaseColor::White),
            _ => {} // Ignorar códigos no soportados.
        }
    }
}

/// Fragmento de texto con un estilo uniforme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Texto visible (sin secuencias de escape).
    pub text: String,
    /// Estilo aplicado.
    pub style: Style,
}

/// Línea descompuesta en spans estilados.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StyledLine {
    /// Fragmentos en orden.
    pub spans: Vec<Span>,
}

impl StyledLine {
    /// Parsea una línea con secuencias ANSI SGR en spans estilados.
    pub fn parse(input: &str) -> StyledLine {
        let mut spans: Vec<Span> = Vec::new();
        let mut style = Style::default();
        let mut text = String::new();
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // ¿Es una secuencia CSI `\x1b[ ... m`?
                if chars.peek() == Some(&'[') {
                    chars.next(); // consumir '['
                    let mut params = String::new();
                    let mut terminated = false;
                    for pc in chars.by_ref() {
                        if pc == 'm' {
                            terminated = true;
                            break;
                        }
                        params.push(pc);
                    }
                    if terminated {
                        // Cerrar el span actual antes de cambiar estilo.
                        if !text.is_empty() {
                            spans.push(Span {
                                text: std::mem::take(&mut text),
                                style,
                            });
                        }
                        apply_params(&mut style, &params);
                        continue;
                    }
                }
                // Escape no reconocido: descartar el byte ESC.
                continue;
            }
            text.push(c);
        }

        if !text.is_empty() {
            spans.push(Span { text, style });
        }
        StyledLine { spans }
    }

    /// Texto plano sin estilos (útil para logging/matching).
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

fn apply_params(style: &mut Style, params: &str) {
    if params.is_empty() {
        style.reset();
        return;
    }
    for part in params.split(';') {
        if let Ok(code) = part.parse::<u32>() {
            style.apply_sgr(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_single_span() {
        let line = StyledLine::parse("hello world");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "hello world");
        assert_eq!(line.spans[0].style, Style::default());
    }

    #[test]
    fn parses_bright_cyan_banner() {
        // Igual que rush::ansi::cosmos_banner.
        let line = StyledLine::parse("\x1b[96m== cosmos ==\x1b[0m");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "== cosmos ==");
        assert_eq!(line.spans[0].style.fg, Color::Bright(BaseColor::Cyan));
    }

    #[test]
    fn reset_returns_to_default() {
        let line = StyledLine::parse("\x1b[95mmag\x1b[0mplain");
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].style.fg, Color::Bright(BaseColor::Magenta));
        assert_eq!(line.spans[1].style, Style::default());
        assert_eq!(line.plain(), "magplain");
    }

    #[test]
    fn bold_and_color_combined() {
        let line = StyledLine::parse("\x1b[1;31mboom\x1b[0m");
        assert!(line.spans[0].style.bold);
        assert_eq!(line.spans[0].style.fg, Color::Red);
    }

    #[test]
    fn ignores_unknown_codes() {
        let line = StyledLine::parse("\x1b[123mtext");
        assert_eq!(line.plain(), "text");
    }

    #[test]
    fn plain_strips_all_escapes() {
        let line = StyledLine::parse("\x1b[96mRUGUS\x1b[0m;tier=lite");
        assert_eq!(line.plain(), "RUGUS;tier=lite");
    }
}
