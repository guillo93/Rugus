# Architecture

## Posicionamiento — RTOS y OS en un solo codebase

Rugus **es un sistema operativo**. La pregunta interesante no es *"¿es OS
o RTOS?"* — RTOS es una subcategoría de OS, no algo distinto — sino *"¿qué
forma adopta en cada chip donde corre?"*

```
                    Sistemas Operativos (OS)
                            |
       ┌────────────────────┼────────────────────┐
       |                    |                    |
  General-purpose         RTOS              Especializados
   (Linux, BSD,       (FreeRTOS, Zephyr,    (TempleOS,
    Windows, macOS)    Tock, VxWorks)        exokernels)
       ▲                    ▲
       └─── Rugus en ───────┴── Rugus en ────
            Cortex-A,           Cortex-M, AVR,
            RISC-V64 con S      RISC-V32 sin paging
```

| Backend de Rugus | Personalidad | Por qué |
|---|---|---|
| `rugus-arch-cortex-m` | **RTOS** | Sin MMU paginada; single AS; *tasks* en pool estático |
| `rugus-arch-avr` (futuro) | **RTOS minimalista** | Sin alloc dinámico, sin MPU; cooperativo a secas |
| `rugus-arch-riscv32` (futuro, ESP32-C3) | **RTOS** | RV32 sin paginación |
| `rugus-arch-cortex-a` (RPi 3B+, AArch64) | **RTOS hoy → OS general-purpose** | Backend operativo: el `Scheduler<A>` de `rugus-core` corre y se **preempta por el Generic Timer** (G5, `examples/rpi3-preempt`), igual léxico que el M. Camino futuro: MMU + EL0/EL1 → procesos aislados, kernel/user real |
| `rugus-arch-riscv64` (futuro) | **OS general-purpose** | S-mode + paginación SV39/SV48 |

**No es exclusivo de Rugus.** Zephyr y seL4 navegan terreno parecido. Lo
distintivo es que Rugus lo abraza **desde el diseño**, no como evolución
tardía: el trait `Arch` define la mínima superficie común; cada backend
aporta lo específico de su ISA sin contaminar al `core`. No hay
`if cfg!(target_arch)` esparcido en cada función — el aislamiento vive
en el trait.

> Llamar "RTOS" a Rugus en Cortex-M no le quita ser OS. Es como llamar
> "kart" a un kart: sigue siendo vehículo.

Implicación práctica para contribuidores: cuando trabajes en `rugus-core`,
pregunta *"¿esta lógica vale para todos los backends?"*. Si la respuesta
es no, va al `rugus-arch-<isa>` correspondiente — no al `core` con un
`cfg`.

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

### QEMU como red de seguridad

Cada arch backend incluye un ejemplo `qemu-<arch>` que arranca en QEMU sin
necesidad de placa física. Permite:

- **Validar el scheduler** sin quemar tiempo de flash + reset on HW.
- **Iterar en CI** sin runners con hardware conectado.
- **Reproducir bugs reportados** con un comando, no con una placa prestada.

Comandos típicos (post G1):

```bash
# Cortex-M7 sintético — la MPS2 AN500 emula un M7 base
qemu-system-arm -machine mps2-an500 -cpu cortex-m7 \
                -nographic -semihosting-config enable=on,target=native \
                -kernel target/thumbv7em-none-eabihf/release/examples/qemu-cortex-m

# Cortex-A53 (Raspberry Pi 4, futuro)
qemu-system-aarch64 -machine raspi4b -nographic \
                    -kernel target/aarch64-unknown-none/release/examples/qemu-cortex-a

# RISC-V32 (futuro)
qemu-system-riscv32 -machine virt -nographic \
                    -bios none -kernel ...
```

QEMU **no sustituye** las pruebas on-target — diferencias en cache,
timings de IRQ y peripheral models son reales. Pero el 80 % de los bugs
de lógica se cazan en QEMU antes de tocar la placa.

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
