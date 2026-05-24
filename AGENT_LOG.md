# Agent Log — Rugus

Bitácora de sesiones de agentes IA que han trabajado en este repositorio.
Orden cronológico **ascendente** (más reciente abajo). Formato por entrada:

```
## YYYY-MM-DD — <modelo> — <título corto>

**Scope:** qué se tocó.
**Decisiones clave:** las no obvias y por qué.
**Estado al cerrar:** qué compila, qué falta, qué queda pendiente.
**Próximo agente que toque esto:** sugerencias accionables.
```

---

## 2026-05-24 — Claude Opus 4.7 — Génesis G0: workspace multi-arch + STM32F7 blink

**Contexto:** Repo creado desde cero hoy. Surge como spin-off de
`guillo93/Panel-smartH`, donde inicialmente el kernel estaba acoplado al
firmware del panel. El owner clarificó la visión: **el kernel es una
plataforma multi-arquitectura propia (Rugus)**, no firmware acoplado.
El panel queda como primer consumidor en su propio repo.

**Scope:** Bootstrap completo del repositorio Rugus desde directorio vacío
hasta workspace funcional con primer ejemplo blink.

**Entregado:**

- **Workspace** con 5 crates:
  - `rugus-core` — arch-agnostic; trait `Arch` + scheduler stub + syscall ABI v0.1 + `Errno`.
  - `rugus-arch-cortex-m` — impl `Arch` para Cortex-M (stub G0; real en G1).
  - `rugus-hal` — solo traits, `#![forbid(unsafe_code)]`: `GpioPin`, `SerialPort`.
  - `rugus-hal-stm32f7` — impl GPIO para los 4 LEDs DISCO; features por variante (f767/f769/f779).
  - `rugus-runtime` — panic-probe + defmt-rtt + entry macro re-export para Cortex-M.
- **Ejemplo** `examples/blink-stm32f769-disco/`: binario standalone con su
  propio `memory.x`, `.cargo/config.toml`, README. Toggle LD1 (PJ13)
  usando `rugus_hal_stm32f7::LedPin` vía trait `rugus_hal::GpioPin`.
- **Docs** completas: `ARCHITECTURE`, `ROADMAP` (G0..G∞), `PORTING`,
  `HAL_TRAITS`, `SECURITY_MODEL`, `SYSCALL_ABI`, `INVARIANTS`,
  `agent-memory/{README,project,preferences}`.
- **Infra**: dual licensing MIT/Apache-2.0, CONTRIBUTING, rustfmt.toml,
  CI con matrix por target (preparada para crecer cuando se añadan archs).

**Decisiones clave (no obvias):**

1. **`rugus-hal` separado de `rugus-core`**. La HAL es solo traits y no
   depende del kernel. Esto permite que un driver third-party use los
   traits sin arrastrar el scheduler — útil para que el ecosistema crezca
   sin atarse al runtime de Rugus.
2. **Trait `Arch` minimalista**. Solo primitivas comunes a casi cualquier
   ISA (context switch, critical section, WFI, reset). Features específicas
   (MPU regions, MMU tables, PMP entries) viven en cada `rugus-arch-<isa>`
   como API propia. Evita el over-abstraction trap de querer un trait
   universal de aislamiento.
3. **`examples/` con `memory.x` y `.cargo/config.toml` por ejemplo**. Cada
   placa tiene su mapa y target distintos. Tener un `memory.x` global
   sería falsamente reutilizable.
4. **CI con matrix por target desde día 1**, aunque solo tenga uno
   inicialmente. La estructura permite añadir `thumbv6m-none-eabi`,
   `riscv32imac-unknown-none-elf`, etc., con un solo edit YAML.
5. **`rugus-arch-cortex-m::switch_context` es stub no-op en G0**. Suficiente
   para que `rugus-core` compile con un backend real; impl real (PendSV +
   naked ASM en `.itcm`) llega en G1 cuando el scheduler la necesite.
6. **`rugus-hal-stm32f7` con feature `stm32f769` por defecto**. Otros chips
   de la familia (f767/f779) tienen feature propia. El consumidor selecciona.

**Estado al cerrar:**

- Workspace **escrito**, no ejecutado. `cargo build` no se ha corrido aún.
- El primer push activará CI; es probable que clippy con `-D warnings`
  falle por stubs con código no-usado. Aceptable; iterar como primer ciclo.
- Plan: dos commits (bootstrap en `main`, génesis en
  `feat/genesis-g0-cortex-m-stm32f7`), abrir PR para revisar la estructura
  completa.

**Próximo agente que toque esto:**

1. **Verificar que `cargo build --workspace` pasa.** Si falla por nombres
   de campo del PAC `stm32f7 = 0.15.1` (e.g. `gpiojen()` puede haberse
   renombrado), corregir en `crates/rugus-hal-stm32f7/src/gpio.rs`.
2. **Flashear `blink-stm32f769-disco`** y confirmar LD1 parpadea + logs
   RTT visibles.
3. **G1**: empezar por `rugus-hal-stm32f7::rcc` (HSE 25 MHz → PLL 216 MHz +
   AHB/APB dividers + I/D-Cache enable). Luego `fmc` para SDRAM. Luego
   `rugus-core::sched` cooperativo. Luego `rugus-arch-cortex-m::switch_context`
   real con PendSV + naked ASM.
4. **No tocar `docs/SYSCALL_ABI.md`** sin coordinar. Los IDs son ABI estable
   a partir de G2 (post-MPU).
5. **Coordinar con `guillo93/Panel-smartH`**: ese repo se va a refactorizar
   como consumidor delgado de Rugus en un PR aparte. Cualquier cambio
   breaking en `rugus-hal-stm32f7` antes de G2 implica avisar.
