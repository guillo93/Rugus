---

---

## 2026-05-25 — Agent — G3 cerrado: F407 dual-blink + app-sandbox (PR feat/g3-f407-complete)

**Scope:** G3 completion — optional "muy bien hecho" items on STM32F407G-DISC1.

**Entregado:**

- `examples/dual-blink-stm32f407g-disco` — LD4/LD6 cooperative scheduler, heap 32 KiB SRAM.
- `examples/app-sandbox-stm32f407g-disco` — MPU + syscalls + MemManage kill (no SDRAM).
- Scripts `verify-{dual-blink,app-sandbox}-stm32f407g-disco.sh` con `PROBE_RS_PROBE` default F407.
- ROADMAP G3 cerrado; CHANGELOG [0.4.0]; `docs/boards/stm32f407g-disco.md` ampliado.

**Verificación HW (STM32F407G-DISC1, probe `0483:3752:…`):**

- `./tools/verify-blink-stm32f407g-disco.sh` — **8/8 PASS**
- `./tools/verify-dual-blink-stm32f407g-disco.sh` — **10/10 PASS**
- `./tools/verify-app-sandbox-stm32f407g-disco.sh` — **12/12 PASS**

**Recomendación F103 vs G4:**

- **G4 primero** si F769 es la placa producto (Panel-smartH): ETH + smoltcp + HTTPS necesitan F7.
- **F103 en paralelo** como proyecto fin de semana para ampliar ecosistema (Cortex-M3, sin FPU/MPU).
- Sin cambios en `rugus-arch-cortex-m` — MPU G2 funciona en M4 sin refactor.

**Próximo agente:** Merge PR → tag `v0.4.0`; iniciar G4 (red/TLS) o F103 downscale según decisión usuario.

## 2026-05-25 — Agent — G3 HW verified: STM32F407G-DISC1 LD4 blink (PR #21)

**Verificación HW (usuario confirmó LD4 verde):**

- LD4 (green, PD12) blink visible on STM32F407G-DISC1 @ 168 MHz SYSCLK.
- `./tools/verify-blink-stm32f407g-disco.sh` — **8/8 PASS**.
- Dual ST-Link lab: `PROBE_RS_PROBE=0483:3752:066EFF575353667267172509`.

**Docs:** `docs/boards/stm32f407g-disco.md` — PROBE_RS_PROBE example for F407 + F769.

**Próximo agente:** Merge PR #21; cerrar checkboxes G3 en ROADMAP si aplica.

## 2026-05-25 — Agent — G3 kickoff: STM32F407G-DISC1 blink (PR feat/g3-stm32f407g-disco)

**Scope:** G3 inicio — `rugus-hal-stm32f4`, `examples/blink-stm32f407g-disco`,
`docs/boards/`, agent-memory + ROADMAP, verify script.

**Entregado:**

- Crate `rugus-hal-stm32f4`: GPIO (LD3–LD6 PD12–PD15), RCC HSE 8 MHz → PLL 168 MHz.
- Ejemplo `blink-stm32f407g-disco`: LD4 toggle + defmt RTT.
- Docs: `docs/boards/{README,stm32f407g-disco,stm32f103c8-bluepill}.md`.
- `project.md` / `ROADMAP.md`: F407 Discovery como G3; F103 Blue Pill post-G3.

**Verificación HW (2026-05-25):**

- `probe-rs list` → dos ST-Link V2-1 (`0483:374b`, `0483:3752`).
- `cargo build --workspace --release --target thumbv7em-none-eabihf` — OK.
- `./tools/verify-blink-stm32f407g-disco.sh` — build/clippy/defmt **5/8 PASS**;
  flash/RTT bloqueado: probe `3752` → `JtagGetIdcodeError` (SWD sin target);
  probe `374b` es F769 (page write falla con algo F407). **Re-flashear con solo
  F407 USB conectado** y `PROBE_RS_PROBE=0483:3752:066EFF575353667267172509`.

**Próximo agente:** Confirmar blink LD4 en F407; cerrar checkboxes G3 en ROADMAP.

## 2026-05-25 — Agent — G2 cerrado en main (PR #19 merge)

**Git:** `main` @ dc26239 (merge PR #19).

**Release:** tag `v0.3.0` en origin.

**Verificación (main, HW STM32F769I-DISCO):**

- `cargo build --workspace --release --target thumbv7em-none-eabihf` — OK.
- `cargo fmt --all --check` — OK.
- `./tools/verify-dual-blink-stm32f769-disco.sh` — **10/10 PASS**.
- `./tools/verify-app-sandbox-stm32f769-disco.sh` — **12/12 PASS**.
- `./tools/verify-blink-stm32f769-disco.sh` — **8/8 PASS**.

**Próximo agente:** G3 — STM32F407G-DISC1 (`feat/g3-stm32f407g-disco`).



## 2026-05-25 — Agent — G1 cerrado en main (PR #16 merge)

**Git:** `main` alineado con `origin/main` @ 71488e4 (merge PR #16).

**Verificación (main, HW STM32F769I-DISCO):**

- `cargo build --workspace --release --target thumbv7em-none-eabihf` — OK.
- `cargo clippy --workspace --all-targets --target thumbv7em-none-eabihf -- -D warnings` — OK.
- `./tools/verify-dual-blink-stm32f769-disco.sh` — **10/10 PASS** (SDRAM OK, tasks A/B).
- `./tools/verify-blink-stm32f769-disco.sh` — build/clippy OK; flash RTT falló por
  `interface is busy` (probe en uso por dual-blink concurrente); re-ejecutar solo.

**Release:** CHANGELOG [0.2.0], tag `v0.2.0`, ROADMAP → próximo G2.

**Próximo agente:** G2 — MPU (`rugus-arch-cortex-m::mpu`), luego syscalls SVC.

---

## 2026-05-25 — Composer — G2 completo: MPU + SVC + sandbox (PR feat/g2-mpu-sandbox)

**Scope:** G2 cierre: `rugus-arch-cortex-m::mpu`, syscalls SVC, fault handlers, `app-sandbox-stm32f769-disco`, verify script, docs.

**Entregado:**

- MPU 8 regiones (Drivers/SDRAM/kernel RAM/flash/app stack) con `PRIVDEFENA` y atributos normal memory (WB).
- SVC handler + dispatch ABI v0.1 (`YieldNow`, `TaskId`; `SleepMs`/IPC stub `Einval`).
- Exception handlers: MemoryManagement/BusFault/UsageFault/HardFault → report domain+PC, kill task.
- `sched::spawn_user`, `kill_current_and_resume`, fix `pick_next` no re-elegir tarea actual.
- Ejemplo sandbox: kernel (priv) + good app (SVC yield) + bad app (MemManage @ 0x4000_0000).

**Verificación HW:**

- `./tools/verify-app-sandbox-stm32f769-disco.sh` — **12/12 PASS**.
- `./tools/verify-dual-blink-stm32f769-disco.sh` — **10/10 PASS** (regresión sched OK).

**Próximo agente:** G3 — STM32F407G-DISC1 (`feat/g3-stm32f407g-disco`).

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

---

## 2026-05-24 — Claude Opus 4.7 — Posicionamiento RTOS↔OS + referencias + QEMU

**Contexto:** El owner consultó la opinión de otro agente IA sobre cómo
construir un OS desde cero (mapa genérico tipo RISC-V + QEMU + C + xv6).
La comparación con Rugus llevó a clarificar dos puntos importantes que
no estaban explícitos en los docs:

1. **Rugus es un OS, no algo distinto a un OS.** "RTOS" es una
   subcategoría de OS, no una alternativa. Decir "Rugus en Cortex-M es
   técnicamente un RTOS" no le quita ser OS.
2. **Rugus = un solo codebase, dos personalidades según el chip.** RTOS
   en MCUs (sin MMU paginada), OS general-purpose en SoCs (con MMU).
   Este es el ángulo diferenciador frente a Zephyr (RTOS que añade
   features de OS) y seL4 (microkernel para ambos pero como kernel
   distinto). Rugus lo abraza desde el día 1 via trait `Arch`.

**Scope:**

- `README.md`: nueva sección "Qué es Rugus (y qué no)" con tabla
  RTOS↔OS por arch + frase de posicionamiento como tagline. Añadida
  sección "Referencias canónicas" con xv6, Phil Opp, OSDev, OSTEP,
  Tock, Hubris, Embassy, seL4 + manuales ARM/RISC-V.
- `docs/ARCHITECTURE.md`: nueva sección "Posicionamiento — RTOS y OS en
  un solo codebase" con diagrama de taxonomía y tabla de personalidades
  por backend. Ampliada "Estrategia de testing" con subsección "QEMU
  como red de seguridad" explicando cómo cada arch backend incluirá un
  ejemplo `qemu-<arch>` para CI sin HW.

**Decisiones clave:**

1. **No declarar QEMU como sustituto de HW.** El doc lo dice explícito:
   "QEMU no sustituye pruebas on-target". El 80 % de bugs de lógica se
   cazan ahí; el 20 % restante (cache, timings IRQ, peripheral models)
   requiere placa real.
2. **No editar `docs/agent-memory/preferences.md`** para añadir esto. Es
   un punto de posicionamiento del producto, no una preferencia del owner
   sobre el agente. Va en los docs públicos.
3. **Referencias en README, no en doc aparte.** Si alguien llega al repo
   por primera vez, ver xv6, Phil Opp y Tock en el README le da contexto
   inmediato de qué cultura técnica está mirando.

**Estado al cerrar:** PR #1 de Rugus actualizada con el commit de
posicionamiento. La PR queda más fuerte como mensaje al lector externo.

---

## 2026-05-24 — Claude Opus 4.7 — Fix CI rota + higiene open source

**Contexto:** Tras los pushes anteriores, las 5 checks de CI fallaron.
El owner preguntó qué hacer. Diagnóstico local revela bugs reales (no
solo warnings de stubs como prediqué).

**Bugs encontrados y arreglados:**

1. **`stm32f7 = 0.15.1` no tiene feature `critical-section`**. Asumido
   por error; el PAC no expone esa feature en la 0.15.x. Removido de
   `Cargo.toml` workspace y de `rugus-arch-cortex-m/Cargo.toml`.
2. **`#[defmt::timestamp]` no es atributo, es macro**. El API de defmt
   0.3+ cambió respecto a versiones antiguas: `defmt::timestamp!(...)`
   con expresión, no atributo sobre función. Corregido en
   `rugus-runtime/src/lib.rs`.
3. **Features per-part en `rugus-hal-stm32f7`** (`stm32f767`, `stm32f769`,
   `stm32f779`) eran alias informativos sin efecto. Renombradas/
   documentadas: el PAC agrupa por die (`stm32f7x9` cubre F769/F779),
   no por part-number. Removida `stm32f767` que no encaja en ese die.
4. **Formato `cargo fmt`**: tres líneas en `gpio.rs` excedían 100 cols
   (BSRR writes) y fueron reformateadas automáticamente.

**Validación local (todos pasan):**

- `cargo build --workspace --target thumbv7em-none-eabihf` ✅
- `cargo build --workspace --release --target thumbv7em-none-eabihf` ✅
- `cargo fmt --all -- --check` ✅
- `cargo clippy --workspace --all-targets --target thumbv7em-none-eabihf -- -D warnings` ✅
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --target thumbv7em-none-eabihf` ✅

**Higiene open source añadida (en el mismo commit):**

- `SECURITY.md` — política de vulnerabilidades (GitHub Security
  Advisories como canal preferido, email fallback, SLAs por severidad,
  scope in/out claramente listado).
- `CODE_OF_CONDUCT.md` — Contributor Covenant 2.1 (estándar de facto
  en proyectos open source serios).
- `CHANGELOG.md` — formato Keep a Changelog con entrada `[0.1.0]`
  documentando G0.

**Discusión paralela con el owner sobre licenciamiento:** confirmamos
que el dual MIT/Apache-2.0 actual es el correcto para un kernel embedded
en 2026 (estándar Rust ecosystem, máxima adopción). Linux es GPL por
razones históricas de 1991; replicarlo hoy sería contraproductivo. No
hubo cambio de licencia.

**Estado al cerrar:**

- CI debería ir verde tras este push.
- PR #1 sigue bloqueada por **el ruleset del owner que exige
  `require_signed_commits`**. Yo no configuré esa regla; viene del
  ruleset que él añadió por separado. Soluciones documentadas para él:
  desactivar la regla (rápido), o configurar firma GPG/SSH y reescribir
  los commits (correcto a largo plazo).

**Próximo agente que toque esto:**

1. Verificar que CI quedó verde tras el push.
2. Esperar a que el owner decida la cuestión de firmas.
3. Si CI pasa y firmas se resuelven, PR mergeable.

---

## 2026-05-24 — Claude Opus 4.7 — G0 cerrado en HW + hygiene templates merged

**Scope:** Cierre formal del hito G0 con validación en hardware real, y
merge del PR de templates/badges/dependabot.

**Lo que pasó (cronológico):**

1. **PR #9** (templates + badges + dependabot) mergeado a `main` por rebase.
2. **Tag `v0.1.0`** creado en `main` apuntando al commit del génesis G0
   completo. **GitHub Release** publicado como pre-release con notas
   extraídas del CHANGELOG.
3. **`enforce_admins: true`** activado en branch protection — el owner
   queda sin bypass; cualquier cambio a `main` debe ir por PR + CI verde.
4. **GitHub Discussions** habilitadas.
5. **12 labels OSS** creados (`kind:*`, `prio:*`, `status:*`).
6. **Validación en HW real (clave):** el owner conectó la
   STM32F769I-DISCO por USB ST-LINK. Instalé `probe-rs-tools 0.31.0` vía
   `cargo install`. `probe-rs list` detectó `STLink V2-1`. Intenté
   `cargo run --release` y el linker falló con `cannot find linker
   script memory.x` — **bug que faltó en el commit génesis**: no incluí
   el `build.rs` canónico de cortex-m-rt que copia `memory.x` a `OUT_DIR`.

   **Fix:** creado `examples/blink-stm32f769-disco/build.rs` con el setup
   estándar (`include_bytes!("memory.x")` + `OUT_DIR` + `rustc-link-search`).
   Tras el fix, segundo intento de flash funcionó:

   ```
   Running `probe-rs run --chip STM32F769NIHx ...`
   Finished in 0.96s
   0 [INFO ] rugus blink @ STM32F769I-DISCO, HSI 16 MHz default
   0 [INFO ] LD1 (PJ13) configured; toggling at ~1 Hz
   ```

   Owner confirmó: **LD1 (PJ13) parpadea ~1 Hz físicamente en la placa**.
   **G0 cerrado en HW.**

7. Checkboxes G0 marcados en `docs/ROADMAP.md`.

**Decisiones clave:**

1. **No re-pushear a `v0.1.0` con el fix del build.rs**. El tag apunta al
   estado del génesis G0 sin validar; el build.rs fix va en
   `[Unreleased]` del CHANGELOG y entra al próximo release (probablemente
   `v0.1.1` patch o se incluye en `v0.2.0` con G1). Razón: tags
   inmutables son contrato; reescribir un tag publicado rompe a quien lo
   haya clonado.
2. **Mantener warnings cosméticos** sobre `target-feature=+vfp4` y
   `+fp-armv8d16sp`. rustc 1.95 los marca como deprecated; el firmware
   funciona. Limpieza en otro PR aparte (no en este fix puntual).
3. **CI no detectó el bug del `build.rs`** porque `cargo build
   --workspace` aparentemente skipea el link del binario en algunos
   estados de cache. Investigar en G1 — quizás añadir `cargo run -p
   blink-stm32f769-disco` a un job de CI con `--no-run` o ejecución en
   QEMU para asegurar que el link siempre se ejerce.

**Estado al cerrar:**

- **Rugus G0 cerrado oficialmente.** Workspace compila, CI verde, release
  publicado, firmware probado en HW.
- PR `fix/blink-build-rs` abierto con el fix + checkboxes + AGENT_LOG +
  CHANGELOG. Pendiente CI + merge.
- Dependabot ya activo: hay un PR auto-abierto bumpeando
  `actions/checkout` a v6.

**Próximo agente que toque esto:**

1. Mergear este PR cuando CI pase (todo es config/docs + un build.rs trivial).
2. Mergear o revisar el PR de Dependabot.
3. **Empezar G1** — issue #2 (RCC HSE → PLL 216 MHz) es el primer paso.
   Recomendación: design-doc corto en el body del PR, código en commits
   del mismo PR, validar en HW con multímetro o smoke test
   `cortex_m::asm::delay(216_000_000)` ≈ 1 segundo.
4. Considerar PR aparte para limpiar warnings de `target-feature` (rustc
   1.95 ya no acepta `vfp4` ni `fp-armv8d16sp`). Probable fix: dejar solo
   `target-cpu=cortex-m7` en `.cargo/config.toml` y dejar que el target
   `thumbv7em-none-eabihf` deduzca el FPU.

---

## 2026-05-24 — Composer — Debug G1 blink: hang en VOSRDY (RCC)

**Scope:** `crates/rugus-hal-stm32f7/src/rcc.rs`, verificación HW con
probe-rs 0.31.0 + ST-Link en STM32F769I-DISCO.

**Cronología de debug:**

1. **Síntoma:** Tras flash (~0.85 s) LD1 no parpadea y RTT vacío con firmware G1
   (HSE→PLL 216 MHz + cache). G0 (HSI 16 MHz, sin RCC) sí arranca y RTT OK.
2. **Bisección RTT:** Logs `defmt` entre pasos de `rcc::init` → hang en
   `configure_voltage_scale` esperando `CSR1.VOSRDY`.
3. **Registros en HW:** Tras reset `CR1=0xC000` (VOS Scale 1), `CSR1=0`
   (`VOSRDY=0`). Re-escribir Scale 1 o bajar a Scale 2 cambia CR1 pero
   `VOSRDY` nunca se pone a 1 → bucle infinito.
4. **Over-drive sí funciona** sin poll de VOSRDY cuando ya estamos en Scale 1:
   `ODRDY`/`ODSWRDY` pasan; HSE, PLL y switch a 216 MHz completan.
5. **Fix:** En `configure_voltage_scale`, solo programar VOS y esperar VOSRDY
   si CR1.VOS **no** es ya Scale 1. Tras reset no tocar VOS ni bloquear en
   VOSRDY.

**Causa raíz:** Esperar `VOSRDY` tras re-escribir Scale 1 en cold boot
(CR1 ya en Scale 1, CSR1.VOSRDY=0). El regulador no completa la secuencia
“ready” sin transición real de VOS.

**Verificación post-fix (probe-rs run):**

```
INFO  rugus blink @ STM32F769I-DISCO, SYSCLK 216 MHz
INFO  LD1 (PJ13) configured; toggling at ~1 Hz
```

**Comando de flash:**

```bash
cd examples/blink-stm32f769-disco
cargo build --release --target thumbv7em-none-eabihf
probe-rs run --chip STM32F769NIHx --log-format full --rtt-scan-memory \
  ../../target/thumbv7em-none-eabihf/release/blink-stm32f769-disco
```

**Estado al cerrar:** Firmware G1 arranca en HW; RTT confirma 216 MHz y loop
LD1. Pendiente confirmación visual del usuario de que LD1 parpadea ~1 Hz.

**Próximo agente:** Confirmar blink en placa; luego commit/PR G1. No avanzar
fases G1 restantes hasta merge verificado en HW.

---

## Verificación automatizada pipeline (2026-05-24T23:40-05:00)

Checklist:

- [x] build OK (`cargo build --workspace --release --target thumbv7em-none-eabihf`)
- [x] clippy OK (`cargo clippy --workspace --all-targets --target thumbv7em-none-eabihf -- -D warnings`)
- [x] flash OK (`probe-rs run`, ELF relinked con `defmt.x` vía build del ejemplo)
- [x] RTT: SYSCLK 216 MHz
- [x] RTT: LD1 configured
- [x] no fault detected

Notas:

- `cargo build --workspace` desde la raíz **no** aplica `rustflags` de
  `examples/blink-stm32f769-disco/.cargo/config.toml`; el ELF queda sin
  sección `.defmt` y `probe-rs` falla con «no `.defmt` section». Rebuild del
  paquete blink (o script) corrige el enlace.
- RTT capturado (~25 s, exit 124 por `timeout` esperado).
- ST-LINK detectado: `probe-rs list` → STLink V2-1 `0483:374b`.
- LED: usuario confirmó parpadeo manual; RTT automatizado también OK.

Script: `tools/verify-blink-stm32f769-disco.sh`

---

## 2026-05-25 — Composer — G1 completo: scheduler + dual-blink + FMC/heap

**Scope:** G1 cierre: `fmc`, `heap`, `sched`, PendSV, `dual-blink-stm32f769-disco`,
scripts verify, docs ROADMAP/CHANGELOG.

**Decisiones clave:**

1. **PendSV sin `bl` a Rust** — las llamadas `bl` dentro del handler pisaban LR
   (EXC_RETURN); switch vía literales `RUGUS_SWITCH_PREV/NEXT`.
2. **Bootstrap primera tarea** — `start_first` salta directo al PC del frame
   sintético; PendSV solo para `yield` cooperativo.
3. **SDRAM verify** — secuencia BSP ST presente pero readback falla en placa;
   ejemplo usa heap fallback en SRAM interna hasta afinar FMC/MPU (G2).

**Estado al cerrar:**

- `./tools/verify-blink-stm32f769-disco.sh` — 8/8 PASS.
- `./tools/verify-dual-blink-stm32f769-disco.sh` — 9/10 (SDRAM verify WARN en HW).
- RTT dual-blink: task A/B alternan LD1/LD2 sin HardFault.

**Próximo agente:** Afinar `fmc::init` verify (GPIO/timing/MPU); G2 MPU + syscalls.

---

## 2026-05-25 — Composer — Fix SDRAM verify en F769I-DISCO (PR #16)

**Problema:** `fmc::init` completaba secuencia FMC pero verify devolvía readback=0;
dual-blink caía a heap fallback en SRAM interna (9/10 en verify script).

**Diagnóstico (HW + RTT):**

1. RTT: `VerifyFailed`, readback=0, sdcr1=0x19E4 (registro FMC OK).
2. PG8 (SDCLK): `moder=0`, `afr=0` tras init → pines FMC nunca muxeados.
3. Causa raíz: macro `af_port!` escribía vía `&dp.GPIOx` del PAC; en este
   crate los accesos efectivos requieren `GPIOx::ptr()` (como `gpio.rs` / LEDs).

**Fix aplicado (`crates/rugus-hal-stm32f7/src/fmc.rs`):**

- GPIO FMC: `GPIOx::ptr()` + AF12, pull-up, very-high speed (BSP ST).
- `RBURST`/`RPIPE` movidos a `SDCR1` (bank 1), no `SDCR2`.
- Deshabilitar FMC NOR bank1 (`BCR1.MBKEN`) para evitar bloqueo bus especulativo.
- Init SDRAM antes de D-cache en dual-blink; verify condicional si cache off.

**Verificación HW:**

- RTT: `SDRAM OK @ 0xC0000000`, heap en SDRAM 256 KiB.
- `./tools/verify-dual-blink-stm32f769-disco.sh` — **10/10 PASS**.

**Commit:** en rama `feat/g1-scheduler-dual-blink`, push a PR #16.

