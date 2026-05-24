# Contributing to Rugus

Rugus es un kernel embebido en Rust puro `no_std`, multi-arquitectura por
diseño. La barra técnica es alta por la naturaleza del trabajo (bare-metal,
ASM ocasional, MPU/MMU, multiples ISAs), no por elitismo.

## Setup

```powershell
rustup toolchain install stable
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools --locked
cargo build --workspace
```

Para ejecutar el ejemplo blink en una STM32F769I-DISCO:

```powershell
cd examples\blink-stm32f769-disco
cargo run
```

## Reglas duras

1. **Pure Rust `no_std`.** Cero FFI a C en `rugus-core`, `rugus-arch-*`,
   `rugus-hal`, `rugus-runtime`. Si una pieza no existe en Rust, la
   escribimos. Excepción posible solo en `rugus-hal-<chip>` cuando un vendor
   exija blob propietario (raro; documentar).
2. **`unsafe` por capa.**
   - `rugus-arch-*`, `rugus-runtime`, `rugus-hal-*`: `unsafe` permitido con
     `// SAFETY:` justificando cada bloque.
   - `rugus-core`: `unsafe` solo en context-switch ASM, MPU writes, syscall
     dispatch.
   - `rugus-hal` (solo traits): `#![forbid(unsafe_code)]`.
3. **Dependencias unidireccionales** entre crates:
   ```
   examples → rugus-hal-<chip> → rugus-hal → rugus-core → rugus-arch-<isa>
                                                ↓
                                          rugus-runtime
   ```
4. **No prometer una arquitectura sin un ejemplo en HW.** Añadir
   `rugus-arch-riscv` sin que `examples/blink-esp32c3` parpadee es deuda
   y promesa rota.
5. **Cambios en `rugus-core::syscall`** requieren actualizar
   `docs/SYSCALL_ABI.md` en el mismo PR y bumpear la constante
   `ABI_VERSION` si el cambio es breaking.
6. **`cargo fmt` + `cargo clippy --workspace --all-targets -- -D warnings`**
   debe pasar antes de commit. CI lo verifica.

## Cómo añadir un nuevo chip / arch

Ver [`docs/PORTING.md`](docs/PORTING.md). Resumen:

1. **Arch nueva**: crear `crates/rugus-arch-<isa>/` implementando el trait
   `Arch` de `rugus-core`. Añadir target a `rust-toolchain.toml` y matrix de
   CI.
2. **Chip nuevo de una arch existente**: crear
   `crates/rugus-hal-<chip-family>/` implementando los traits de `rugus-hal`.
3. **Demostrarlo**: añadir `examples/<demo>-<board>/` con su `memory.x`,
   `.cargo/config.toml` y un binario que parpadee algo verificable.

## Estilo de commit

Convención: `<tipo>(<scope>): <resumen imperativo>`.

Tipos: `feat`, `fix`, `docs`, `refactor`, `chore`, `ci`, `test`, `perf`, `sec`, `port`.

Scopes: `core`, `arch-cortex-m`, `hal`, `hal-stm32f7`, `runtime`, `examples`,
`docs`, `ci`. Para portado nuevo, usar `port(<chip>)`.

Ejemplos:

- `feat(core): scheduler cooperativo round-robin`
- `port(rp2040): primer blink en Pico`
- `sec(arch-cortex-m): hardening de region MPU del kernel`

## Para agentes IA que asistan

Lectura obligatoria al empezar sesión (en este orden):

1. `docs/agent-memory/project.md` — visión, decisiones bloqueadas.
2. `docs/agent-memory/preferences.md` — preferencias del owner.
3. Entrada más reciente de `AGENT_LOG.md`.
4. `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`.
5. Si tocas porting: `docs/PORTING.md`.
6. Si tocas seguridad: `docs/SECURITY_MODEL.md`, `docs/INVARIANTS.md`.

Al cerrar sesión: añade entrada en `AGENT_LOG.md` (modelo, fecha, scope,
decisiones, estado, próximo paso). Si descubres una preferencia nueva del
owner, añade entrada `P-N` a `docs/agent-memory/preferences.md`.
