# Security Model

Modelo de seguridad de Rugus. Aplica a todos los chips con MPU/MMU/PMP;
los chips sin aislamiento HW reciben best-effort y se marca como tal en
el README del `rugus-hal-<chip>` correspondiente.

## Modelo de amenazas

| Vector | Probabilidad | Impacto | Mitigación primaria |
|--------|--------------|---------|---------------------|
| Bug en app que escala a kernel | Alta | Alto | MPU domains, `#![forbid(unsafe_code)]` en apps, syscalls validadas |
| Update OTA malicioso | Media | Crítico | Firma Ed25519, rollback dual-bank, watchdog |
| Atacante en LAN (consumidor red) | Alta | Medio | TLS + cert pinning desde `rugus-tls` |
| Acceso físico SWD breve | Media | Alto | RDP nivel 2 tras producción, secretos solo descifrables por kernel |
| Side-channel HW (DPA) | Muy baja | Bajo | Fuera de alcance (no es un HSM) |

## Dominios MPU (Cortex-M)

Cuando la arch implementa MPU, `rugus-core` define **4 dominios** que el
arch backend mapea a regiones HW (8 regiones en Cortex-M7):

| Dominio | Privilegio | Permisos típicos |
|---------|------------|------------------|
| `Kernel`   | Privileged | RWX en kernel .text/.data, no accesible desde user |
| `Drivers`  | Privileged | RW periféricos, X drivers HAL |
| `Services` | User       | RW arena del servicio, RX su .text |
| `App`      | User       | RW arena de la app activa (remapeada en switch), RX su .text |

Adicionalmente, regiones especiales: framebuffer compartido (RW user+priv),
**Secrets** (priv R-- solo, user ---).

## Reglas de `unsafe`

| Crate | Política |
|-------|----------|
| `rugus-core`         | `unsafe` solo en switch ASM, MPU/MMU writes, syscall dispatch |
| `rugus-arch-*`       | `unsafe` libre con `// SAFETY:` |
| `rugus-hal` (traits) | `#![forbid(unsafe_code)]` |
| `rugus-hal-*` (impls)| `unsafe` permitido, encapsulado |
| `rugus-runtime`      | `unsafe` permitido (vector table, panic) |
| Apps consumidoras    | `#![forbid(unsafe_code)]` recomendado |

## Boot verificado

Disponible en chips con flash suficiente (≥256 KB libres para bootloader).

```
┌─────────────────────────────────────────────┐
│ Bootloader Rugus (primer sector flash)      │
│  - Lee header del slot activo (A o B)       │
│  - Verifica firma Ed25519                   │
│  - Verifica SHA-256 (HW si disponible)      │
│  - Si OK: salta a entrypoint                │
│  - Si KO: prueba otro slot, recovery si     │
│           ambos KO                          │
└─────────────────────────────────────────────┘
```

Implementación viva en `crates/rugus-bootloader/` (futuro, hito G6).

## OTA dual-bank con rollback

1. App descarga firmware nuevo al slot inactivo.
2. Verifica firma localmente.
3. Marca slot inactivo como `next-boot tentative`.
4. Reset.
5. Bootloader arranca slot nuevo.
6. Si el firmware nuevo no marca `confirmed` en N segundos (watchdog
   independiente), bootloader vuelve al slot anterior.

## Secretos

`rugus-core` define un trait `SecretStore`:

```rust
pub trait SecretStore {
    fn sign(&self, payload: &[u8]) -> Result<Signature, Error>;
    fn rng(&self, buf: &mut [u8]) -> Result<(), Error>;
    // No expone `get_key()`. Las claves no salen del store.
}
```

El backend lo provee el chip:
- STM32F7: BKPSRAM bajo región MPU dedicada.
- RP2040: OTP fuses + lockable XIP region.
- Cortex-A: TrustZone TZASC + secure world (futuro lejano).

## Invariantes auditadas

Ver [`INVARIANTS.md`](INVARIANTS.md). Cada PR que pueda violar uno debe
demostrar en su descripción que el invariante se mantiene.
