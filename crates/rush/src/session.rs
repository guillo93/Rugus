//! Autenticación de canal de la consola (F6.1) — challenge-response HMAC.
//!
//! ## Modelo
//!
//! La consola es un plano de control: ejecuta verbos que reinician, reconfiguran
//! o actúan sobre el dispositivo. Sin autenticar, cualquiera con acceso al
//! transporte (serial/BLE/—en el futuro—LAN) podría hacerlo. Este módulo exige
//! que el operador **pruebe** conocimiento de una clave precompartida (PSK)
//! antes de aceptar verbos privilegiados.
//!
//! Protocolo (challenge-response, la PSK nunca cruza el cable):
//!
//! 1. `knock` → el dispositivo genera un nonce aleatorio de un solo uso y
//!    responde `challenge <nonce-hex>`.
//! 2. `prove <proof-hex>` con `proof = HMAC-SHA256(PSK, nonce)` → el dispositivo
//!    recalcula y compara en tiempo constante; éxito ⇒ sesión autenticada.
//! 3. `lock` → cierra la sesión.
//! 4. `enroll <psk-hex>` → aprovisiona la PSK **una sola vez** (fábrica); si ya
//!    está aprovisionada, se rechaza (re-provisión exige factory reset físico).
//!
//! ## Separación de secretos
//!
//! rush **no** ve la PSK ni calcula HMAC: delega en la personalidad por punteros
//! de función ([`AuthHooks`]). Así el secreto queda en la capa de placa (flash
//! protegido) y la consola universal permanece libre de material criptográfico.

use crate::ansi::Write;

/// Longitud del nonce de reto en bytes (128 bits de un solo uso).
pub const NONCE_LEN: usize = 16;
/// Longitud de la prueba HMAC-SHA256 en bytes.
pub const PROOF_LEN: usize = 32;
/// Longitud máxima de PSK aceptada en `enroll` (= tamaño de bloque HMAC).
pub const PSK_MAX: usize = 64;
/// Ventana de inactividad de la sesión (ms). Tras ella se exige re-autenticar.
pub const SESSION_TIMEOUT_MS: u32 = 300_000;

/// Ganchos que la personalidad aporta para la autenticación. La PSK y el HMAC
/// viven aquí, no en rush.
#[derive(Clone, Copy)]
pub struct AuthHooks {
    /// `true` si el dispositivo ya tiene PSK aprovisionada.
    pub provisioned: fn() -> bool,
    /// Calcula `HMAC-SHA256(PSK, nonce)` y lo compara en tiempo constante con
    /// `proof`. Devuelve `true` solo si coincide. rush nunca ve la PSK.
    pub verify_proof: fn(nonce: &[u8], proof: &[u8]) -> bool,
    /// Aprovisiona la PSK (una sola vez). Devuelve `true` si la persistió.
    pub enroll: fn(psk: &[u8]) -> bool,
    /// Rellena `buf` con bytes aleatorios para el nonce.
    pub random_nonce: fn(buf: &mut [u8]),
    /// Reloj monotónico en ms (para expirar la sesión por inactividad).
    pub now_ms: fn() -> u32,
}

/// Estado de la sesión de consola.
#[derive(Clone, Copy)]
enum State {
    /// Sin autenticar: solo IDENTIFY/orbit/handshake permitidos.
    Locked,
    /// Reto emitido, esperando `prove`. Guarda el nonce de un solo uso.
    AwaitingProof { nonce: [u8; NONCE_LEN] },
    /// Autenticada; `last_ms` marca la última actividad (para timeout).
    Authenticated { last_ms: u32 },
}

/// Sesión de consola: máquina de estados de autenticación. Una por consola.
pub struct Session {
    state: State,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Crea una sesión cerrada (sin autenticar).
    pub const fn new() -> Self {
        Self {
            state: State::Locked,
        }
    }

    /// `true` si la sesión está autenticada y no ha expirado por inactividad.
    pub fn is_authenticated(&mut self, hooks: &AuthHooks) -> bool {
        if let State::Authenticated { last_ms } = self.state {
            let now = (hooks.now_ms)();
            if now.wrapping_sub(last_ms) <= SESSION_TIMEOUT_MS {
                // Refresca la actividad: comandos seguidos no expiran.
                self.state = State::Authenticated { last_ms: now };
                return true;
            }
            // Expiró: vuelve a cerrar.
            self.state = State::Locked;
        }
        false
    }

    /// Procesa `knock`: genera nonce y responde el reto.
    pub fn knock(&mut self, out: &mut dyn Write, hooks: &AuthHooks) {
        if !(hooks.provisioned)() {
            crate::paint::warn(out, "sin PSK — usa enroll <psk-hex> (una vez)");
            return;
        }
        let mut nonce = [0u8; NONCE_LEN];
        (hooks.random_nonce)(&mut nonce);
        self.state = State::AwaitingProof { nonce };
        // Etiqueta en oro (autoridad) y el reto en cian (dato).
        crate::paint::tint(out, rugus_ui::Role::Focus, "challenge ");
        if rugus_ui::color() {
            let _ = out.write_str(rugus_ui::CIAN);
        }
        write_hex(out, &nonce);
        if rugus_ui::color() {
            let _ = out.write_str(rugus_ui::RESET);
        }
        let _ = out.write_str("\r\n");
    }

    /// Procesa `prove <proof-hex>`: verifica y abre sesión si coincide.
    pub fn prove(&mut self, proof_hex: &[u8], out: &mut dyn Write, hooks: &AuthHooks) {
        let State::AwaitingProof { nonce } = self.state else {
            crate::paint::warn(out, "sin reto activo — usa knock primero");
            return;
        };
        // Consume el nonce pase lo que pase (un solo uso, anti-replay).
        self.state = State::Locked;
        let mut proof = [0u8; PROOF_LEN];
        if decode_hex(proof_hex, &mut proof) != Some(PROOF_LEN) {
            crate::paint::err(out, "prueba mal formada");
            return;
        }
        if (hooks.verify_proof)(&nonce, &proof) {
            self.state = State::Authenticated {
                last_ms: (hooks.now_ms)(),
            };
            crate::paint::ok(out, "auth ok — sesión abierta");
        } else {
            crate::paint::err(out, "auth fail — prueba inválida");
        }
    }

    /// Procesa `lock`: cierra la sesión.
    pub fn lock(&mut self, out: &mut dyn Write) {
        self.state = State::Locked;
        crate::paint::ok(out, "sesión cerrada");
    }

    /// Procesa `enroll <psk-hex>`: aprovisiona la PSK una sola vez.
    pub fn enroll(&mut self, psk_hex: &[u8], out: &mut dyn Write, hooks: &AuthHooks) {
        if (hooks.provisioned)() {
            crate::paint::warn(out, "enroll: ya aprovisionada (factory reset para cambiar)");
            return;
        }
        let mut psk = [0u8; PSK_MAX];
        let Some(len) = decode_hex(psk_hex, &mut psk) else {
            crate::paint::err(out, "enroll: psk-hex mal formada");
            return;
        };
        if len == 0 {
            crate::paint::err(out, "enroll: psk vacía");
            return;
        }
        if (hooks.enroll)(&psk[..len]) {
            crate::paint::ok(out, "enroll ok — PSK aprovisionada");
        } else {
            crate::paint::err(out, "enroll: error al persistir");
        }
    }
}

/// Escribe `bytes` como hex minúscula al sink de la consola.
fn write_hex(out: &mut dyn Write, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in bytes {
        let pair = [HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]];
        // SAFETY: HEX solo contiene ASCII => UTF-8 válido.
        let _ = out.write_str(unsafe { core::str::from_utf8_unchecked(&pair) });
    }
}

/// Decodifica hex ASCII en `out`; devuelve el nº de bytes escritos, o `None` si
/// la entrada es inválida (longitud impar, dígito no hex, o excede `out`).
fn decode_hex(s: &[u8], out: &mut [u8]) -> Option<usize> {
    if s.len() % 2 != 0 || s.len() / 2 > out.len() {
        return None;
    }
    let mut i = 0;
    while i < s.len() {
        let hi = hex_val(s[i])?;
        let lo = hex_val(s[i + 1])?;
        out[i / 2] = (hi << 4) | lo;
        i += 2;
    }
    Some(s.len() / 2)
}

/// Valor de un dígito hex ASCII (mayúsc/minúsc), o `None`.
fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_hex_ok() {
        let mut out = [0u8; 4];
        assert_eq!(decode_hex(b"00ffa510", &mut out), Some(4));
        assert_eq!(out, [0x00, 0xff, 0xa5, 0x10]);
    }

    #[test]
    fn decode_hex_rejects_bad() {
        let mut out = [0u8; 4];
        assert_eq!(decode_hex(b"abc", &mut out), None); // impar
        assert_eq!(decode_hex(b"zz", &mut out), None); // no hex
        let mut small = [0u8; 1];
        assert_eq!(decode_hex(b"0011", &mut small), None); // no cabe
    }
}
