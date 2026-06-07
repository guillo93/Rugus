//! Cableado de la autenticación de canal (F6.1) para la personalidad lite.
//!
//! `rush` aporta la máquina de estados ([`rush::Session`]) pero delega el
//! secreto y la criptografía en la personalidad vía [`rush::AuthHooks`]. Aquí se
//! construyen esos ganchos para la Blue Pill:
//!
//! - `provisioned` / `enroll` → almacén de PSK en flash ([`crate::psk`]).
//! - `verify_proof` → HMAC-SHA256(PSK, nonce) recalculado con `rugus-crypto` y
//!   comparado en tiempo constante. La PSK solo la lee este módulo y nunca sale.
//! - `random_nonce` → CSPRNG software sembrado con el contador de ciclos DWT y
//!   el uptime (F103 no tiene TRNG por hardware).
//! - `now_ms` → reloj monotónico del arch (para expirar la sesión).

use core::sync::atomic::{AtomicU32, Ordering};

use rand_core::RngCore;
use rugus_crypto::{ct_eq, hmac_sha256, SoftwareRng};
use rush::AuthHooks;

/// Longitud máxima de PSK (= bloque HMAC-SHA256), igual que en `psk`.
const PSK_MAX: usize = 64;

/// Contador monotónico que perturba cada siembra del CSPRNG, garantizando que
/// dos `knock` consecutivos en el mismo milisegundo produzcan nonces distintos.
static NONCE_TWEAK: AtomicU32 = AtomicU32::new(0);

/// Construye los ganchos de autenticación para `rush`.
pub fn hooks() -> AuthHooks {
    AuthHooks {
        provisioned: crate::psk::provisioned,
        verify_proof,
        enroll: crate::psk::enroll,
        random_nonce,
        now_ms: rugus_arch_cortex_m::time::now_ms,
    }
}

/// Recalcula `HMAC-SHA256(PSK, nonce)` con la PSK en flash y lo compara en
/// tiempo constante con `proof`. `rush` nunca ve la PSK: solo este módulo la
/// lee, y solo para alimentar el HMAC. Devuelve `false` si no hay PSK.
fn verify_proof(nonce: &[u8], proof: &[u8]) -> bool {
    let mut psk = [0u8; PSK_MAX];
    let len = crate::psk::read_psk(&mut psk);
    if len == 0 {
        return false;
    }
    let expected = hmac_sha256(&psk[..len], nonce);
    // Borra la copia en pila de la PSK tras usarla (higiene de secreto).
    psk.fill(0);
    ct_eq(&expected, proof)
}

/// Rellena `buf` con bytes del CSPRNG software, sembrado con entropía fresca:
/// contador de ciclos DWT (jitter de arranque/actividad) ⊕ uptime ⊕ tweak
/// monotónico. No es un TRNG, pero basta para nonces de un solo uso que solo
/// deben ser impredecibles e irrepetibles entre retos.
fn random_nonce(buf: &mut [u8]) {
    let cycles = cortex_m::peripheral::DWT::cycle_count() as u64;
    let ms = rugus_arch_cortex_m::time::now_ms() as u64;
    let tweak = NONCE_TWEAK.fetch_add(1, Ordering::Relaxed) as u64;
    let seed = (cycles << 32)
        ^ (ms.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        ^ tweak.wrapping_mul(0xD1B5_4A32_D192_ED03);
    let mut rng = SoftwareRng::seed_from_u64(seed);
    rng.fill_bytes(buf);
}
