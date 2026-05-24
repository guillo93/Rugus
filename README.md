# Rugus

> **OS Rust puro multi-arquitectura que se comporta como RTOS en MCUs y
> como OS general-purpose en SoCs, usando un único codebase.**
>
> Diseñado para crecer poco a poco hasta convertirse en un sistema
> operativo sofisticado, sin perder de vista el control total sobre el
> hardware.

## Qué es Rugus (y qué no)

**Rugus no es una categoría nueva. Es un kernel que cambia de
personalidad según el chip donde corre.**

| Chip / Arch | Rugus se comporta como | Por qué |
|---|---|---|
| Cortex-M0+/M4/M7/M33, RISC-V32 sin paginación | **RTOS** | Sin MMU paginada → single address space, *tasks* en vez de procesos POSIX |
| AVR (ATmega) | **RTOS minimalista** | Aún más pequeño, sin allocator dinámico, sin MPU |
| Cortex-A53 (Raspberry Pi 4, EL1, MMU), RISC-V64 con S-mode | **OS general-purpose** | MMU presente → procesos aislados, page tables, kernel/user split real |

Esto no es exclusivo nuestro — Zephyr y seL4 navegan terreno parecido —
pero Rugus lo abraza **desde el diseño, no como evolución tardía**:
`rugus-core` define el trait `Arch` con la mínima superficie común; cada
backend (`rugus-arch-cortex-m`, futuros `rugus-arch-cortex-a`,
`rugus-arch-riscv`, `rugus-arch-avr`) aporta lo específico de su ISA.

Llamar "RTOS" a Rugus en Cortex-M no le quita ser OS; igual que llamar
"kart" a un kart no le quita ser vehículo. Lo que sí evitamos es la
trampa común de meter `if cfg!(target_arch)` en cada función — el aislamiento
vive en el trait, no en el código.

## Estado

**Génesis (mayo 2026).** Estructura inicial del workspace y primer backend
funcional: ARM Cortex-M sobre STM32F7. Ejemplo `blink-stm32f769-disco`
flashea, parpadea LD1 y emite logs `defmt` por SWD/RTT.

## Principios

1. **Rust puro `no_std`.** Cero FFI a C en el núcleo. Stack pure-Rust de
   pies a cabeza: `smoltcp` para red, `embedded-tls` para TLS,
   `embedded-graphics`/Slint para GUI, drivers escritos directamente sobre
   los PACs `svd2rust`.
2. **Multi-arquitectura por diseño.** Trait `Arch` aísla lo específico de
   cada CPU (context switch, MPU/MMU, IRQ controller, timers). Empezamos
   con Cortex-M7; la estructura permite añadir Cortex-M0+/M4/M33, ARMv8-A,
   AVR/ATmega y RISC-V sin reescribir el `core`.
3. **HAL por traits.** `rugus-hal` define interfaces (`GpioPin`, `SerialPort`,
   `EthMac`, `Display`, `Crypto`…); cada familia de chips las implementa en
   su propio crate (`rugus-hal-stm32f7`, futuros `rugus-hal-rp2040`,
   `rugus-hal-esp32c3`, etc.).
4. **Seguridad como pilar, no como feature.** MPU/MMU obligatorios donde el
   chip los tenga; dominios de privilegio aislados; boot verificado y OTA
   con rollback en chips con flash suficiente.
5. **Crecimiento iterativo.** No se promete soporte para una arquitectura
   hasta que existe un ejemplo que parpadea en hardware real.

## Arquitecturas / chips planificados

| Arch | Chip ejemplar | Estado |
|------|---------------|--------|
| ARMv7E-M (Cortex-M7) | STM32F769NIH6 | **génesis** — blink en HW |
| ARMv7-M (Cortex-M4) | STM32F4xx | planeado tras Fase 2 |
| ARMv6-M (Cortex-M0+) | RP2040 | planeado tras Fase 3 |
| ARMv8-M Main (Cortex-M33) | nRF5340 / STM32L5 | tras Fase 4 |
| AVR 8-bit | ATmega328P | exploratorio (sin MPU, sin alloc) |
| RISC-V RV32IMAC | ESP32-C3 | tras Fase 5 |
| ARMv8-A 64-bit | Raspberry Pi 4/5 | meta a largo plazo (modo EL1, MMU) |

Cada chip vive en `crates/rugus-hal-<chip-family>/` y aporta su propio
ejemplo en `examples/<demo>-<board>/`.

## Estructura

```
Rugus/
├── crates/
│   ├── rugus-core/             scheduler, IPC, syscall ABI, MPU-agnóstico
│   ├── rugus-arch/             trait Arch + tipos comunes  (futuro split)
│   ├── rugus-arch-cortex-m/    impl Arch para ARMv7-M / v7E-M / v8-M
│   ├── rugus-hal/              traits HAL (GpioPin, SerialPort, …)
│   ├── rugus-hal-stm32f7/      impl HAL para STM32F7
│   └── rugus-runtime/          panic + defmt-rtt + entry macros
├── examples/
│   └── blink-stm32f769-disco/  primer ejemplo en HW real
├── docs/
│   ├── ARCHITECTURE.md
│   ├── ROADMAP.md
│   ├── PORTING.md              cómo añadir una nueva arch o un nuevo chip
│   ├── HAL_TRAITS.md           contrato de los traits de la HAL
│   ├── SECURITY_MODEL.md
│   ├── SYSCALL_ABI.md
│   ├── INVARIANTS.md
│   └── agent-memory/           memoria para agentes IA que asistan
└── AGENT_LOG.md
```

## Quickstart — blink en STM32F769I-DISCO

```powershell
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools --locked
cd examples\blink-stm32f769-disco
cargo run
```

Debería verse:

```
INFO  [rugus-core] init v0.1.0
INFO  [blink] LD1 toggling on PJ13
```

## Primeros consumidores

- [`guillo93/Panel-smartH`](https://github.com/guillo93/Panel-smartH) — panel
  de pared smart-home; primer producto basado en Rugus.

## Roadmap resumido

Ver [`docs/ROADMAP.md`](docs/ROADMAP.md) para el plan completo. Hitos:

- **G0 (génesis):** workspace, traits, Cortex-M7 blink. ← **ahora**
- **G1:** scheduler cooperativo + clocks + heap básico.
- **G2:** MPU + dominios + syscalls SVC.
- **G3:** segundo chip Cortex-M (RP2040 o STM32F4).
- **G4:** red (smoltcp) + TLS (embedded-tls) + crypto HW abstraction.
- **G5:** primer ejemplo Cortex-A (Raspberry Pi 4 EL1).
- **G∞:** OS sofisticado (apps nativas, IPC rich, sistema de paquetes, IA
  embebida opcional).

## Referencias canónicas

Lecturas y proyectos sobre los que Rugus se apoya o de los que aprende:

- **[xv6 (MIT)](https://github.com/mit-pdos/xv6-riscv)** — Unix didáctico ~10K líneas, código limpio y comentado. Lectura obligatoria para entender un kernel completo de cabo a rabo.
- **[Writing an OS in Rust](https://os.phil-opp.com/)** (Philipp Oppermann) — tutorial moderno x86_64; ideas portables al modelo Rugus aunque el target sea distinto.
- **[OSDev Wiki](https://wiki.osdev.org/)** — referencia diaria; cubre x86, ARM, RISC-V con nivel desigual pero útil.
- **[Operating Systems: Three Easy Pieces](https://pages.cs.wisc.edu/~remzi/OSTEP/)** (OSTEP) — teoría sin la cual no entiendes lo que estás programando.
- **[Tock OS](https://github.com/tock/tock)** — RTOS Rust para Cortex-M con foco en seguridad por dominios; inspiración directa para el modelo MPU+capacidades de Rugus.
- **[Hubris (Oxide)](https://github.com/oxidecomputer/hubris)** — microkernel Rust para Cortex-M en producción; referencia de cómo se hace IPC riguroso en embedded.
- **[Embassy](https://github.com/embassy-rs/embassy)** — runtime async Rust embedded; no es nuestro modelo (Rugus es preemptivo, no async-first), pero su HAL es excelente referencia.
- **[seL4](https://sel4.systems/)** — microkernel formalmente verificado; aspiración a largo plazo en el modelo de seguridad.
- **ARMv7-M Architecture Reference Manual**, **ARMv8-A ARM**, **RISC-V Privileged Spec** — los manuales del hardware son no-negociables.

## Licencia

MIT OR Apache-2.0.
