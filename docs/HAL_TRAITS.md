# HAL Traits — Contrato público

`rugus-hal` define las interfaces que cualquier `rugus-hal-<chip>` debe
implementar. Este documento es el contrato; los traits aquí descritos son
estables a partir de Rugus 1.0.

## Filosofía

- **Mínimo común denominador, no abstracción universal.** Si un trait
  necesita 12 generics para cubrir todos los chips, está mal diseñado.
  Mejor que cada chip exponga sus features avanzadas como API propia.
- **Compatible con `embedded-hal`** donde tenga sentido. Re-exportamos los
  traits de `embedded-hal` v1 para que crates third-party funcionen.
- **Async opcional, no obligatorio.** Algunas operaciones (DMA transfers,
  network) tienen variantes `async`; las síncronas siguen siendo el caso
  base.

## Traits núcleo (G0-G1)

### `GpioPin`

```rust
pub trait GpioPin {
    type Error;
    fn set_high(&mut self) -> Result<(), Self::Error>;
    fn set_low(&mut self) -> Result<(), Self::Error>;
    fn toggle(&mut self) -> Result<(), Self::Error>;
    fn is_high(&self) -> Result<bool, Self::Error>;
}
```

### `SerialPort`

```rust
pub trait SerialPort {
    type Error;
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error>;
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}
```

## Traits planeados (G1-G4)

| Trait        | Hito | Notas |
|--------------|------|-------|
| `Timer`      | G1   | tick-based, monotónico |
| `Rng`        | G2   | si el chip tiene TRNG, lo usa; si no, PRNG con seed inicial |
| `EthMac`     | G4   | nivel MAC, smoltcp consumirá esta interfaz |
| `Display`    | G4   | framebuffer-style, compatible con embedded-graphics |
| `Crypto`     | G4   | AES/HASH/HMAC; backend HW si el chip lo tiene |
| `BlockDev`   | G5   | SDIO/eMMC/SPI-flash; para FS futuro |

## Capacidades **chip-specific** (no en `rugus-hal`)

Estas capacidades existen solo en algunos chips y se exponen como API
propia del crate `rugus-hal-<chip>`. No están en `rugus-hal` porque no
tiene sentido pedirlas a todos:

| Capacidad           | Chips |
|---------------------|-------|
| Chrom-ART (DMA2D)   | STM32F7, F4 (algunos) |
| JPEG HW codec       | STM32F7 (raro), STM32H7 |
| OTFDEC firewall     | STM32H7 high-end |
| PIO state machines  | RP2040 |
| Ultra Low Power     | STM32L4/L5 |
| Hardware Trigonometry (CORDIC) | STM32G4 |

Cada uno con su módulo en `rugus-hal-<chip>::` y documentado en el README
del crate.

## Versionado del contrato

- Cambios **aditivos** a un trait (default impl, método nuevo con default):
  bump minor.
- Cambios **breaking** (método sin default, signature change): bump major.
- Trait nuevo: bump minor.

Cada cambio actualiza este documento en el mismo PR.
