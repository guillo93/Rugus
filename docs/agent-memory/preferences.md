# Owner Preferences — Rugus

Preferencias explícitas o inferidas del owner del proyecto, formuladas como
reglas accionables para agentes IA. Cada entrada incluye **Why** y
**How to apply**.

Añade entradas nuevas (P-N+1) cuando el owner corrija un enfoque o
confirme una elección no obvia. **No editar entradas existentes** salvo
refinamiento de formulación; las reglas tienen historia.

---

## P-1 — Rust puro, sin FFI a C, con paciencia para timelines largos

**Why:** Frase textual del owner (2026-05-24): *"todo en rust. no importa
cuanto tiempo pase, con paciencia se logra."* Valor declarado: control
total, mantenibilidad, consistencia del ecosistema.

**How to apply:**
- Preferir crates pure-Rust `no_std`: `smoltcp`, `embedded-tls`,
  `embedded-graphics`, `rustls`, `heapless`.
- **Nunca** sugerir wrappers FFI a C como atajo: LVGL, lwIP, mbedtls,
  FreeRTOS C-API.
- Si solo hay opción C, advertirlo y proponer reescritura en Rust como
  tarea futura, no aceptar FFI silenciosamente.
- No defender alternativas con "es más rápido al MVP" si chocan con esta
  preferencia. Aceptar timelines largos como diseño, no como problema.

---

## P-2 — Estructura nivel "ingeniero senior PhD" desde Fase 0

**Why:** Frase textual del owner (2026-05-24): *"estructura nivel ingeniero
de software nivel senior phd"*. Quiere un repo del que se sienta orgulloso
enseñar.

**How to apply:**
- Workspace Cargo con `[workspace.dependencies]` centralizadas, perfiles
  `dev`/`release`/`release-dev`.
- `docs/` con docs separados densos (ARCHITECTURE, ROADMAP, SECURITY_MODEL,
  HAL_TRAITS, INVARIANTS, SYSCALL_ABI, PORTING…), no un README inflado.
- CI configurado desde día 1.
- Dependencias entre capas explícitas en `ARCHITECTURE.md`.
- No reducir scope a "minimal hello world"; entregar esqueleto completo
  con stubs documentados.
- Justificar decisiones en doc-comments a nivel módulo/struct, no comentarios
  inline triviales.

---

## P-3 — Aprovechar features hardware específicas del chip

**Why:** El owner eligió kernel propio sobre Zephyr precisamente para
explotar features que un RTOS portable desperdicia. Usar abstracciones
genéricas que oculten capacidades del HW contradice la decisión raíz.

**How to apply:**
- Acceso directo al PAC `svd2rust` en los `rugus-hal-<chip>`; no usar HAL
  community crates portables como `stm32f7xx-hal`.
- Drivers que aprovechan: Chrom-ART (DMA2D), JPEG HW codec, CRYP/HASH
  hardware, TRNG, OTFDEC firewall, PIO state machines (RP2040), CORDIC
  (STM32G4), etc.
- Hot paths (context switch, IRQ entry, syscall dispatch) en memoria
  rápida cuando exista (ITCM en Cortex-M7).
- Capacidades **chip-specific** que no encajen en `rugus-hal` traits van
  como API propia del `rugus-hal-<chip>`, documentadas en
  `docs/HAL_TRAITS.md` § chip-specific extensions.

---

## P-4 — Seguridad como pilar, no como add-on

**Why:** El owner mencionó "aumentado seguridad" como uno de los tres
objetivos del kernel propio (junto a "alto rendimiento" y "todo Rust").

**How to apply:**
- MPU/MMU/PMP obligatorios desde hito G2 donde el chip los tenga.
- Apps con `#![forbid(unsafe_code)]`; `unsafe` confinado a kernel y HAL
  impls con `// SAFETY:` justificado.
- Validación de punteros user-space en todo syscall handler (vía
  `Arch::validate_user_ptr` o equivalente).
- Secretos vía trait `SecretStore` que **nunca** expone `get_key()`.
- Boot verificado Ed25519 + OTA dual-bank con rollback (hito G6) **antes**
  de exponer endpoint OTA público.

---

## P-5 — Multi-arquitectura genuina, sin promesas sin prueba

**Why:** El owner declaró visión multi-arch (2026-05-24): *"se puede
ejecutar en otros chip mas adelante como arm64 o 32 o atmega, y otros mas
hasta alcanzar ser un os mas sofisticado, va escalado poco a poco."*
Riesgo: declarar soporte para N arches sin verificar ninguna degrada el
proyecto a "rust embedded vapor".

**How to apply:**
- No documentar soporte para una arch hasta que `examples/blink-<board>`
  parpadee en HW real.
- Trait `Arch` mínimo común; features específicas (MPU, MMU, PMP) en cada
  `rugus-arch-<isa>`, no obligadas en el trait genérico.
- Por cada arch nueva, actualizar `README.md`, `ROADMAP.md`, `PORTING.md`
  y CI matrix.
- Si una arch resulta inviable (e.g. AVR sin alloc impide IPC con
  `heapless` decente), documentar la limitación en `docs/arch-<isa>.md` y
  no degradar la API del kernel para acomodarla.

---

## P-6 — Idioma de comunicación: español; código y commit messages: inglés

**Why:** Conversaciones del owner consistentemente en español. Convención
embedded mantiene identificadores y commits en inglés por convención
internacional.

**How to apply:**
- Respuestas conversacionales al owner: **español**.
- Commit messages, identifiers de código, mensajes `defmt`: **inglés**.
- Docstrings y comentarios largos: español o inglés, consistente dentro
  del mismo archivo.
- Docs en `docs/`: español (es lo que el owner lee). README puede ser
  español o bilingüe.

---

## P-7 — Memoria de agente en el repo, no solo local

**Why:** Frase textual del owner (2026-05-24): *"esa memroia agente debe
estar tambien en la repo, por eso te la pedi, no que te la guardes en tu
carpeta"*. Quiere que cualquier agente que clone el repo tenga el
contexto, no solo el agente con memoria privada local.

**How to apply:**
- Mantener `docs/agent-memory/{project,preferences}.md` actualizados con
  cualquier cambio relevante.
- `AGENT_LOG.md` en raíz como bitácora cronológica.
- En conflicto entre memoria privada del agente (`~/.claude/...`) y los
  archivos del repo, **el repo es la fuente de verdad**.
- Al detectar una preferencia nueva del owner, escribirla aquí **en el
  mismo PR** que el cambio que la respeta, no en sesión aparte.

---

*Para añadir P-N+1: copia la plantilla anterior, asigna número siguiente,
no renumerar existentes.*
