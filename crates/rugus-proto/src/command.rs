//! Modelo de comandos del host hacia el dispositivo.
//!
//! Refleja el léxico de la shell `rush`. El host valida/asiste pero el parseo
//! "de verdad" ocurre en el dispositivo; por eso los argumentos viajan como
//! texto. [`Command::to_wire`] produce la línea exacta a transmitir.

/// Verbos conocidos del léxico `rush` v1, con ayuda corta para la TUI.
pub const LEXICON: &[(&str, &str)] = &[
    ("cosmos", "info del sistema (sys_info)"),
    ("orbit", "ayuda / lista de comandos"),
    ("ecosystem", "estado del sistema (sys_status)"),
    ("moor", "asociar pin a rol — moor P N rol"),
    ("pulso", "leer GPIO — pulso P N"),
    ("spark", "GPIO alto — spark P N"),
    ("mute", "GPIO bajo — mute P N"),
    ("ripple", "toggle GPIO — ripple P N"),
    ("scout", "escanear bus I2C — scout [bus]"),
    ("sonar", "leer módulo — sonar N"),
    ("schema", "leer clave RFN — schema clave"),
    ("scribe", "escribir clave RFN — scribe clave valor"),
    ("seal", "validar/persistir config"),
    ("nest", "listar módulos"),
    ("hatch", "recargar app — hatch nombre"),
    ("coil", "listar tareas del scheduler"),
    ("anchor", "modo fail-safe"),
    ("ward", "watchdog — ward [kick]"),
    ("IDENTIFY", "firma de descubrimiento del dispositivo"),
];

/// Comando del host. Los verbos conocidos se tipan; el resto es passthrough.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Verbo reconocido del léxico con su línea original (verbo + args).
    Known {
        /// Verbo (p. ej. `cosmos`).
        verb: String,
        /// Línea completa tal cual se transmitirá (sin terminador).
        line: String,
    },
    /// Línea arbitraria (passthrough hacia el dispositivo).
    Raw(String),
}

impl Command {
    /// Construye un comando a partir de la entrada del usuario.
    ///
    /// Recorta espacios; reconoce el verbo si pertenece al léxico.
    pub fn parse(input: &str) -> Command {
        let line = input.trim().to_string();
        let verb = line.split_whitespace().next().unwrap_or("");
        if !verb.is_empty() && is_known_verb(verb) {
            Command::Known {
                verb: verb.to_string(),
                line,
            }
        } else {
            Command::Raw(line)
        }
    }

    /// Atajo para construir el comando IDENTIFY.
    pub fn identify() -> Command {
        Command::Known {
            verb: "IDENTIFY".to_string(),
            line: "IDENTIFY".to_string(),
        }
    }

    /// La línea (sin terminador) que se transmite.
    pub fn line(&self) -> &str {
        match self {
            Command::Known { line, .. } => line,
            Command::Raw(line) => line,
        }
    }

    /// Serializa el comando al cable, terminando en `\r\n`.
    pub fn to_wire(&self) -> Vec<u8> {
        let mut v = self.line().as_bytes().to_vec();
        v.extend_from_slice(b"\r\n");
        v
    }

    /// `true` si el verbo pertenece al léxico conocido.
    pub fn is_known(&self) -> bool {
        matches!(self, Command::Known { .. })
    }
}

/// `true` si `verb` es un verbo del léxico `rush`.
pub fn is_known_verb(verb: &str) -> bool {
    LEXICON.iter().any(|(v, _)| *v == verb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_known_verb() {
        let c = Command::parse("  cosmos  ");
        assert!(c.is_known());
        assert_eq!(c.line(), "cosmos");
    }

    #[test]
    fn keeps_args_in_line() {
        let c = Command::parse("pulso C 13");
        assert_eq!(c.line(), "pulso C 13");
        assert!(c.is_known());
    }

    #[test]
    fn unknown_is_raw() {
        let c = Command::parse("frobnicate now");
        assert_eq!(c, Command::Raw("frobnicate now".to_string()));
        assert!(!c.is_known());
    }

    #[test]
    fn wire_has_crlf() {
        assert_eq!(Command::parse("orbit").to_wire(), b"orbit\r\n".to_vec());
    }

    #[test]
    fn identify_helper() {
        assert_eq!(Command::identify().to_wire(), b"IDENTIFY\r\n".to_vec());
    }
}
