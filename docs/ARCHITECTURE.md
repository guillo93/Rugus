# Architecture

## Principios fundacionales

1. **Capas con dependencias unidireccionales**, sin ciclos vía
   `dev-dependencies`.
2. **`unsafe` confinado** y justificado con `// SAFETY:`. Las apps que
   eventualmente corran sobre Rugus tendrán `#![forbid(unsafe_code)]`.
3. **Multi-arch desde el diseño**: la lógica genérica vive en `rugus-core`
   y depende de un trait `Arch` que cada crate `rugus-arch-<isa>` implementa.
4. **HAL por traits**: lo específico de un chip se reduce a implementar un
   conjunto cerrado de interfaces (`GpioPin`, `SerialPort`, `EthMac`, …).
5. **Sin promesas sin prueba**: no se documenta soporte para una arch o un
   chip hasta que existe un ejemplo en `examples/` que parpadea en HW.

## Diagrama de crates

```
┌─────────────────────────────────────────────────────────┐
│ examples/<demo>-<board>                                 │  binarios
│  - own memory.x, own .cargo/config.toml                 │
│  - dep: rugus-runtime + rugus-hal-<chip> + rugus-core   │
└───────────────────────────┬─────────────────────────────┘
                            │
          ┌─────────────────┴──────────────────┐
          │                                    │
          ▼                                    ▼
┌──────────────────────┐         ┌─────────────────────────┐
│ rugus-hal-<chip>     │ implmt  │ rugus-runtime           │
│ (stm32f7, rp2040…)   │────────▶│ (panic, defmt-rtt, entry│
│  drivers concretos   │ traits  │  reexport por target)   │
└──────────┬───────────┘         └─────────────────────────┘
           │ traits
           ▼
┌──────────────────────┐
│ rugus-hal            │  traits: GpioPin, SerialPort,
│ (interfaces puras)   │  EthMac, Display, Crypto, Timer
└──────────┬───────────┘
           │
           ▼
┌──────────────────────┐         ┌──────────────────────────┐
│ rugus-core           │ usa     │ rugus-arch-<isa>         │
│ scheduler, IPC,      │────────▶│ (cortex-m, futuro:       │
│ syscall ABI, errno,  │ trait   │  cortex-a, riscv, avr)   │
│ memory abstraction   │ Arch    │ context switch ASM,      │
└──────────────────────┘         │ MPU/MMU, IRQ controller  │
                                 └──────────────────────────┘
```

Reglas duras:

- `rugus-core` **no** depende de PAC alguno ni de `cortex-m`. Solo del trait
  `Arch` que vive en sí mismo y que es implementado por crates externos.
- `rugus-hal` (traits) **no** depende de `rugus-core`. Mantenerlo separable
  permite que un proyecto use solo la HAL sin el kernel.
- `examples/*` son **binarios** con su propio `memory.x` y su `.cargo/config.toml`.
  Nunca declarar `[package]` en `examples/` que dependa de otros ejemplos.

## El trait `Arch`

`rugus-core` define (forma esquemática):

```rust
pub trait Arch {
    type Context;                            // estado de tarea (regs, sp)
    type SavedIrq;                            // estado de máscara de IRQs
    const HAS_MPU: bool;                      // ¿se puede aislar dominios?

    /// Cambia al contexto destino. Marca asm/naked.
    unsafe fn switch_context(prev: *mut Self::Context, next: *const Self::Context);

    /// Sección crítica: enmascara IRQs y devuelve handle para restaurar.
    fn enter_critical() -> Self::SavedIrq;
    fn exit_critical(saved: Self::SavedIrq);

    /// Reinicia la CPU.
    fn reset() -> !;
    /// Para idle: detiene el core hasta próxima IRQ.
    fn wait_for_interrupt();
}
```

Cada arch implementa este trait y añade tipos/funciones específicas que
no aparecen en el contrato general (MPU para Cortex-M, MMU para Cortex-A,
PMP para RISC-V) detrás de feature flags o cfg.

## Política de `unsafe` por crate

| Crate | Política |
|-------|----------|
| `rugus-core`         | `unsafe` solo en switch ASM glue, MPU writes, syscall dispatch |
| `rugus-arch-*`       | `unsafe` libre con `// SAFETY:` obligatorio |
| `rugus-hal` (traits) | `#![forbid(unsafe_code)]` |
| `rugus-hal-*` (impls)| `unsafe` permitido, encapsulado en APIs seguras |
| `rugus-runtime`      | `unsafe` permitido (panic, RTT, vector table) |
| `examples/*`         | `#![deny(unsafe_code)]` salvo glue documentado |

CI rechaza `unsafe` añadido fuera de la política.

## Estrategia de testing

| Crate | Host (unit) | QEMU/Renode | On-target |
|-------|-------------|-------------|-----------|
| `rugus-core` | sí (lógica portable) | scheduler en QEMU | latencias |
| `rugus-arch-*` | n/a | sí | MPU faults, switch |
| `rugus-hal` (traits) | sí (mocks) | n/a | n/a |
| `rugus-hal-*` (impls) | con fakes PAC | n/a | smoke por driver |
| `examples/*` | n/a | bringup en QEMU | bringup real |

`rugus-core` se compila también para `--target x86_64-*` en CI para correr
tests host puros. La parte arch-dependiente queda detrás de `cfg`.

## Versionado y estabilidad

| Crate | Estabilidad esperada |
|-------|----------------------|
| `rugus-core`         | API congelada en `1.0` post G2; SemVer estricto |
| `rugus-hal`          | igual; los traits son contrato público |
| `rugus-arch-*`       | API interna; pueden romper entre minor pre-1.0 |
| `rugus-hal-*`        | API estable; los chips no cambian, los drivers sí |
| `rugus-runtime`      | API trivial; bumps menores raros |

## Lo que Rugus **no** quiere ser

- Linux. Sin MMU paginada con `fork()`, sin POSIX, sin ELF dinámico.
- Un RTOS portable a costa de no aprovechar HW. Si un chip tiene Chrom-ART
  o JPEG HW, el driver lo usa, no se contenta con software portable.
- Un runtime de un solo proyecto. Si tras tres consumidores externos el API
  duele, se refactoriza con bump major.
