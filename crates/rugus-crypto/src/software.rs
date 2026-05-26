//! Pure-Rust software crypto backend.

use core::num::Wrapping;

use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256 as Sha256Inner};

use crate::Digest256;

/// Incremental SHA-256 hasher.
pub struct Sha256 {
    inner: Sha256Inner,
}

impl Sha256 {
    /// New hasher.
    pub fn new() -> Self {
        Self {
            inner: Sha256Inner::new(),
        }
    }

    /// Feed bytes.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finalize into a 32-byte digest.
    pub fn finalize(self) -> Digest256 {
        let out = self.inner.finalize();
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&out);
        digest
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// Software CSPRNG (xoshiro256**), seeded once at boot.
///
/// Suitable for TLS nonces when HW TRNG is unavailable. Replace with
/// `rugus-hal-stm32f7::rng` when that driver lands.
pub struct SoftwareRng {
    s: [Wrapping<u64>; 4],
}

impl SoftwareRng {
    /// Seed from a 64-bit value (e.g. DWT cycle counter XOR MAC bytes).
    pub fn seed_from_u64(seed: u64) -> Self {
        let mut s = [Wrapping(0u64); 4];
        let mut z = seed;
        for slot in &mut s {
            z = z.wrapping_add(0x9E3779B97F4A7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
            *slot = Wrapping(x ^ (x >> 31));
        }
        Self { s }
    }

    fn rotl(x: Wrapping<u64>, k: u32) -> Wrapping<u64> {
        let k = k as usize;
        (x << k) | (x >> (64 - k))
    }

    fn next_u64(&mut self) -> u64 {
        let result = Self::rotl(self.s[1] * Wrapping(5), 7) * Wrapping(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = Self::rotl(self.s[3], 45);
        result.0
    }
}

impl RngCore for SoftwareRng {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }

    fn next_u64(&mut self) -> u64 {
        SoftwareRng::next_u64(self)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        let mut chunks = dest.chunks_exact_mut(8);
        for chunk in chunks.by_ref() {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        let rem = chunks.into_remainder();
        if !rem.is_empty() {
            let tail = self.next_u64().to_le_bytes();
            rem.copy_from_slice(&tail[..rem.len()]);
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for SoftwareRng {}
