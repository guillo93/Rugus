//! Handshake `IDENTIFY` — descubrimiento de dispositivos Rugus.
//!
//! El host envía [`IDENTIFY_REQUEST`] (o el byte [`ENQ`]) por el transporte y
//! el dispositivo responde **una línea** con su firma:
//!
//! ```text
//! RUGUS;tier=lite;chip=f103;proto=1;shell=rush;cli=1.0.0
//! ```
//!
//! [`parse_signature`] valida el prefijo `RUGUS;` y extrae los campos. Las
//! firmas que no empiezan por `RUGUS;` se rechazan (no es un dispositivo Rugus).

use std::collections::BTreeMap;
use std::fmt;

/// Texto de solicitud que el host envía para disparar la respuesta IDENTIFY.
pub const IDENTIFY_REQUEST: &str = "IDENTIFY\r\n";

/// Byte de control ENQ (Enquiry) — alternativa de un solo byte a la línea.
pub const ENQ: u8 = 0x05;

/// Prefijo obligatorio de toda firma Rugus.
pub const SIGNATURE_PREFIX: &str = "RUGUS;";

/// Firma de un dispositivo Rugus, parseada de la respuesta IDENTIFY.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Tier del dispositivo (p. ej. `lite`).
    pub tier: String,
    /// Familia de chip (p. ej. `f103`).
    pub chip: String,
    /// Versión del protocolo IDENTIFY.
    pub proto: u32,
    /// Nombre de la shell embebida (p. ej. `rush`).
    pub shell: String,
    /// Versión del léxico CLI (p. ej. `1.0.0`).
    pub cli: String,
    /// Campos adicionales no reconocidos, preservados para forward-compat.
    pub extra: BTreeMap<String, String>,
}

impl Signature {
    /// Etiqueta corta y legible para menús del host.
    pub fn label(&self) -> String {
        format!(
            "RUGUS {} · {} · shell {} · cli {} (proto {})",
            self.tier, self.chip, self.shell, self.cli, self.proto
        )
    }
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Error al parsear una firma IDENTIFY.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// La línea no empieza por `RUGUS;` → no es un dispositivo Rugus.
    NotRugus,
    /// Falta un campo obligatorio (`tier`, `chip`, `proto`, `shell`, `cli`).
    MissingField(&'static str),
    /// El campo `proto` no es un entero válido.
    BadProto,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::NotRugus => write!(f, "no es una firma Rugus (sin prefijo RUGUS;)"),
            SignatureError::MissingField(name) => write!(f, "falta el campo obligatorio `{name}`"),
            SignatureError::BadProto => write!(f, "campo `proto` inválido"),
        }
    }
}

impl std::error::Error for SignatureError {}

/// Parsea una línea de firma IDENTIFY. Tolera `\r`/`\n` finales y espacios.
///
/// Rechaza cualquier línea que no empiece por `RUGUS;`.
pub fn parse_signature(line: &str) -> Result<Signature, SignatureError> {
    let line = line.trim();
    if !line.starts_with(SIGNATURE_PREFIX) {
        return Err(SignatureError::NotRugus);
    }

    // Campos separados por `;`. El primer token es el prefijo `RUGUS`.
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    for token in line.split(';') {
        let token = token.trim();
        if token.is_empty() || token == "RUGUS" {
            continue;
        }
        if let Some((k, v)) = token.split_once('=') {
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
    }

    let take = |fields: &mut BTreeMap<String, String>, key: &'static str| {
        fields.remove(key).ok_or(SignatureError::MissingField(key))
    };

    let tier = take(&mut fields, "tier")?;
    let chip = take(&mut fields, "chip")?;
    let proto_s = take(&mut fields, "proto")?;
    let shell = take(&mut fields, "shell")?;
    let cli = take(&mut fields, "cli")?;
    let proto = proto_s
        .parse::<u32>()
        .map_err(|_| SignatureError::BadProto)?;

    Ok(Signature {
        tier,
        chip,
        proto,
        shell,
        cli,
        extra: fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_signature() {
        let sig =
            parse_signature("RUGUS;tier=lite;chip=f103;proto=1;shell=rush;cli=1.0.0").unwrap();
        assert_eq!(sig.tier, "lite");
        assert_eq!(sig.chip, "f103");
        assert_eq!(sig.proto, 1);
        assert_eq!(sig.shell, "rush");
        assert_eq!(sig.cli, "1.0.0");
        assert!(sig.extra.is_empty());
    }

    #[test]
    fn tolerates_crlf_and_spaces() {
        let sig =
            parse_signature("  RUGUS; tier=lite ; chip=f103; proto=1; shell=rush; cli=1.0.0\r\n")
                .unwrap();
        assert_eq!(sig.chip, "f103");
        assert_eq!(sig.shell, "rush");
    }

    #[test]
    fn preserves_unknown_fields() {
        let sig =
            parse_signature("RUGUS;tier=pro;chip=h7;proto=2;shell=rush;cli=2.0.0;ble=1").unwrap();
        assert_eq!(sig.proto, 2);
        assert_eq!(sig.extra.get("ble").map(String::as_str), Some("1"));
    }

    #[test]
    fn rejects_non_rugus() {
        assert_eq!(
            parse_signature("HELLO;tier=lite"),
            Err(SignatureError::NotRugus)
        );
        assert_eq!(parse_signature("OK\r\n"), Err(SignatureError::NotRugus));
        assert_eq!(parse_signature(""), Err(SignatureError::NotRugus));
    }

    #[test]
    fn rejects_missing_fields() {
        assert_eq!(
            parse_signature("RUGUS;tier=lite;chip=f103"),
            Err(SignatureError::MissingField("proto"))
        );
    }

    #[test]
    fn rejects_bad_proto() {
        assert_eq!(
            parse_signature("RUGUS;tier=lite;chip=f103;proto=x;shell=rush;cli=1.0.0"),
            Err(SignatureError::BadProto)
        );
    }
}
