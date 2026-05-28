//! Rugus Field Notation (`.rfn`) — parser mínimo userland.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use heapless::String;

/// Máximo de entradas en config staging.
pub const MAX_ENTRIES: usize = 32;
/// Longitud máxima de clave/valor.
pub const MAX_FIELD: usize = 48;

/// Tabla clave→valor en RAM (staging antes de `seal`).
pub struct ConfigMap {
    keys: heapless::Vec<String<{ MAX_FIELD }>, MAX_ENTRIES>,
    vals: heapless::Vec<String<{ MAX_FIELD }>, MAX_ENTRIES>,
}

impl ConfigMap {
    /// Mapa vacío.
    pub fn new() -> Self {
        Self {
            keys: heapless::Vec::new(),
            vals: heapless::Vec::new(),
        }
    }

    /// Inserta o actualiza entrada.
    pub fn insert(
        &mut self,
        key: String<{ MAX_FIELD }>,
        val: String<{ MAX_FIELD }>,
    ) -> Result<(), ()> {
        if let Some(i) = self.keys.iter().position(|k| k == &key) {
            self.vals[i] = val;
            return Ok(());
        }
        self.keys.push(key).map_err(|_| ())?;
        self.vals.push(val).map_err(|_| ())
    }

    /// Obtiene valor por clave.
    pub fn get(&self, key: &str) -> Option<&str> {
        let i = self.keys.iter().position(|k| k.as_str() == key)?;
        Some(self.vals[i].as_str())
    }

    /// Iterador clave-valor.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.keys
            .iter()
            .zip(self.vals.iter())
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl Default for ConfigMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Parsea contenido RFN y rellena `map`.
pub fn parse_rfn(input: &str, map: &mut ConfigMap) -> usize {
    let mut count = 0;
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        if key.is_empty() {
            continue;
        }
        let Ok(k) = String::<MAX_FIELD>::try_from(key) else {
            continue;
        };
        let Ok(v) = String::<MAX_FIELD>::try_from(val) else {
            continue;
        };
        if map.insert(k, v).is_ok() {
            count += 1;
        }
    }
    count
}

/// Metadatos mínimos de un paquete `.afr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AfrHeader {
    /// Nombre lógico de la app.
    pub name: String<{ MAX_FIELD }>,
    /// Versión semver corta.
    pub version: String<16>,
}

/// Parsea cabecera `.afr`.
pub fn parse_afr_header(input: &str) -> Option<AfrHeader> {
    let mut map = ConfigMap::new();
    parse_rfn(input, &mut map);
    let name = map.get("app.name")?;
    let version = map
        .get("app.version")
        .unwrap_or("0.0.0");
    Some(AfrHeader {
        name: String::try_from(name).ok()?,
        version: String::try_from(version).ok()?,
    })
}

/// Serializa map a texto RFN en `out`.
pub fn serialize_rfn(map: &ConfigMap, out: &mut [u8]) -> usize {
    let mut pos = 0;
    for (k, v) in map.iter() {
        let mut line: heapless::String<128> = heapless::String::new();
        let _ = line.push_str(k);
        let _ = line.push_str(" = ");
        let _ = line.push_str(v);
        let _ = line.push_str("\n");
        let bytes = line.as_bytes();
        if pos + bytes.len() > out.len() {
            break;
        }
        out[pos..pos + bytes.len()].copy_from_slice(bytes);
        pos += bytes.len();
    }
    pos
}
