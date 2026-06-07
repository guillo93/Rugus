//! Códigos de autenticación de mensaje (MAC) y comparación en tiempo constante.
//!
//! Base del handshake de autenticación de canal de Rugus (F6.1): el dispositivo
//! reta con un nonce y el operador prueba conocimiento de la clave precompartida
//! (PSK) devolviendo `HMAC-SHA256(PSK, nonce)`. El secreto nunca cruza el cable,
//! así que un sniffer pasivo en serial/BLE no aprende la PSK.

use crate::software::Sha256;
use crate::Digest256;

/// Tamaño de bloque de SHA-256 en bytes (entrada de la función de compresión).
const BLOCK: usize = 64;

/// Calcula `HMAC-SHA256(key, msg)` (RFC 2104) sobre el SHA-256 software.
///
/// Si `key` excede el tamaño de bloque (64 B) se reemplaza por su hash, según
/// el estándar. No registra material secreto (sin `defmt` aquí).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Digest256 {
    // Clave normalizada a un bloque: hash si es más larga, cero-padding si no.
    let mut k0 = [0u8; BLOCK];
    if key.len() > BLOCK {
        let kh = {
            let mut h = Sha256::new();
            h.update(key);
            h.finalize()
        };
        k0[..32].copy_from_slice(&kh);
    } else {
        k0[..key.len()].copy_from_slice(key);
    }

    // ipad/opad: bloque XOR 0x36 / 0x5c.
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] = k0[i] ^ 0x36;
        opad[i] = k0[i] ^ 0x5c;
    }

    // inner = SHA256(ipad || msg)
    let inner = {
        let mut h = Sha256::new();
        h.update(&ipad);
        h.update(msg);
        h.finalize()
    };

    // HMAC = SHA256(opad || inner)
    let mut h = Sha256::new();
    h.update(&opad);
    h.update(&inner);
    h.finalize()
}

/// Compara dos slices en tiempo constante respecto a su contenido.
///
/// Devuelve `true` solo si tienen la misma longitud y todos los bytes coinciden.
/// El tiempo no depende de en qué byte difieren (mitiga ataques de temporización
/// al verificar la prueba del operador). Longitudes distintas devuelven `false`
/// sin fuga temporal explotable (el atacante ya conoce la longitud esperada).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}
