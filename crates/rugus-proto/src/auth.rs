//! Autenticación de canal (lado host) — challenge-response HMAC-SHA256 (F6.1).
//!
//! El dispositivo Rugus, ante un `knock`, responde con un reto:
//!
//! ```text
//! challenge <nonce-hex>
//! ```
//!
//! El operador prueba conocimiento de la clave precompartida (PSK) devolviendo
//! `prove <proof-hex>` donde `proof = HMAC-SHA256(PSK, nonce)`. El dispositivo
//! recalcula y compara en tiempo constante. La PSK **nunca** viaja por el cable,
//! así que un sniffer pasivo (serial/BLE/LAN) no la aprende, y el nonce de un
//! solo uso impide replay.
//!
//! Este módulo aporta el cómputo del lado host (rugus-cli). El firmware usa el
//! mismo `rugus_crypto::hmac_sha256`, garantizando interoperabilidad bit a bit.

use rugus_crypto::hmac_sha256;

/// Longitud en bytes de la prueba (HMAC-SHA256 = 32 B).
pub const PROOF_LEN: usize = 32;

/// Calcula la prueba `HMAC-SHA256(psk, nonce)` a partir de bytes crudos.
pub fn compute_proof(psk: &[u8], nonce: &[u8]) -> [u8; PROOF_LEN] {
    hmac_sha256(psk, nonce)
}

/// Calcula la prueba en hex a partir de la PSK y el nonce en hex del reto.
///
/// Devuelve `None` si el nonce no es hex válido. La salida es hex minúscula,
/// lista para enviar como `prove <proof-hex>`.
pub fn compute_proof_hex(psk: &[u8], nonce_hex: &str) -> Option<String> {
    let nonce = decode_hex(nonce_hex)?;
    Some(encode_hex(&compute_proof(psk, &nonce)))
}

/// Codifica bytes a hex minúscula.
pub fn encode_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xF) as u32, 16).unwrap());
    }
    s
}

/// Decodifica una cadena hex (par de dígitos) a bytes. `None` si es inválida.
pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push((hi << 4 | lo) as u8);
        i += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rugus_crypto::{ct_eq, hmac_sha256};

    // RFC 4231 Test Case 2: key="Jefe", data="what do ya want for nothing?".
    #[test]
    fn hmac_rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let expected =
            decode_hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843").unwrap();
        assert!(ct_eq(&mac, &expected));
    }

    // RFC 4231 Test Case 1: key=0x0b*20, data="Hi There".
    #[test]
    fn hmac_rfc4231_case1() {
        let key = [0x0bu8; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        let expected =
            decode_hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7").unwrap();
        assert!(ct_eq(&mac, &expected));
    }

    #[test]
    fn hex_roundtrip() {
        let data = [0x00, 0x0f, 0xa5, 0xff, 0x10];
        let hex = encode_hex(&data);
        assert_eq!(hex, "000fa5ff10");
        assert_eq!(decode_hex(&hex).unwrap(), data);
    }

    #[test]
    fn decode_hex_rejects_odd_and_nonhex() {
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
    }

    // Handshake completo: host calcula la prueba que el dispositivo aceptaría.
    #[test]
    fn handshake_proof_matches_device_side() {
        let psk = b"clave-de-fabrica-32-bytes-aaaaaa";
        let nonce = [0x42u8; 16];
        let nonce_hex = encode_hex(&nonce);
        // Lado host:
        let proof_hex = compute_proof_hex(psk, &nonce_hex).unwrap();
        // Lado dispositivo (recalcula y compara):
        let device = hmac_sha256(psk, &nonce);
        let host = decode_hex(&proof_hex).unwrap();
        assert!(ct_eq(&device, &host));
    }

    #[test]
    fn wrong_psk_fails() {
        let nonce = [0x01u8; 16];
        let good = hmac_sha256(b"real-psk", &nonce);
        let bad = hmac_sha256(b"wrong-psk", &nonce);
        assert!(!ct_eq(&good, &bad));
    }
}
