//! CRC-32 (IEEE 802.3, polinomio reflejado `0xEDB8_8320`).
//!
//! Implementación sin tabla (bit a bit) para no gastar flash en una LUT; el
//! volumen de datos por registro es pequeño (cabecera de 24 B + clave/valor),
//! así que el coste por byte es irrelevante frente a la latencia QSPI.

/// Calcula el CRC-32/ISO-HDLC de `data` (init `0xFFFF_FFFF`, xorout final).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn vector_check() {
        // Vector estándar: CRC-32 de "123456789" = 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(crc32(b""), 0);
    }
}
