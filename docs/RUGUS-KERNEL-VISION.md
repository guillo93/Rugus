# Visión del kernel Rugus — arquitectura por capas

Documento de referencia sobre **qué es el núcleo Rugus**, **cómo se organiza**
y **qué reglas son no negociables** para contribuidores. Complementa
[`ARCHITECTURE.md`](ARCHITECTURE.md), [`INVARIANTS.md`](INVARIANTS.md) y
[`SECURITY_MODEL.md`](SECURITY_MODEL.md).

---

## Metáfora del edificio

Rugus se concibe como un edificio de varios pisos. Cada piso tiene un rol;
los materiales y las normas estructurales cambian según la altura.

### Sótanos y fundamento — el kernel serio

En la base vive el **Trusted Computing Base (TCB) mínimo**: scheduler,
syscalls, validación de punteros, watchdog, manejo de faults y la política
*app que faulta → kernel mata la tarea, no panic global*. Este fundamento debe
ser **antisísmico**: resistir bugs internos, configuración corrupta y ataques
desde capas superiores sin derrumbar el sistema entero.

Analogía: un terremoto no puede tumbar los cimientos porque el resto del
edificio depende de ellos.

### Niveles medios — servicios y conectividad

Sobre el kernel corren **servicios en userland**, drivers fuera del TCB,
parsers de **RFN/AFR**, stack de red (`rugus-net`), TLS y crypto. Aquí
encajan módulos LoRa, BLE (HM-10 u otros), sensores I2C y la lógica de
appliance. Pueden fallar, reiniciarse o sustituirse sin reescribir el sótano.

### Cima — experiencia de usuario

En la parte alta: **`rugus-cli`** (comandos con léxico propio), GUI futura,
apps nativas en `.afr`. Objetivo: **fluido y bonito** — cosmética, ergonomía,
textos amigables. Nada de esto debe filtrarse al kernel.

### Regla de oro

> **Lo de arriba NO entra en el sótano.**

Separación estricta de responsabilidades (*separation of concerns*):

| Capa | Ejemplos permitidos | Prohibido en kernel core |
|------|---------------------|--------------------------|
| Capa 0 (kernel) | syscalls, MPU/MMU, WDT, validación | parsear SD, BLE stack, UI |
| Media | RFN parser, drivers, red | lógica de presentación |
| Alta | rugus-cli, GUI, apps | acceso directo a hardware sin syscall |

Un parser de `.rfn` **nunca** corre en handler de IRQ. Un stack BLE **nunca**
se mergea en `rugus-core` “por comodidad”.

### Open source y disciplina arquitectónica

Rugus es **código abierto**: cualquiera puede leer, clonar y proponer cambios.
Eso **no** significa que todo pueda ir en un solo crate.

- La arquitectura por capas es **obligatoria**, no opcional.
- Contribuidores **no deben** fusionar parsers, UI o stack BLE en el núcleo
  del kernel.
- El review protege el TCB: un PR que expande `rugus-core` con lógica de
  appliance debe rechazarse o redirigirse a userland.
- **Apertura ≠ monolito.** Transparencia del código y minimalismo del
  fundamento van juntos.

---

## Rugus único — no es clon de Linux, Windows ni RTOS genérico

Rugus comparte escala con un RTOS en embebido (pools estáticos, latencia
predecible, footprint acotado), pero **no copia** FreeRTOS, Zephyr ni Linux:

| Aspecto | RTOS/Linux típico | Rugus |
|---------|-------------------|-------|
| Syscalls | API del RTOS o POSIX | **Propios**, ABI versionada (`SYSCALL_ABI.md`) |
| Personalidades | Un solo perfil o `#ifdef` | **lite / full / power** por backend `Arch` |
| Seguridad | Opcional o add-on | **Separación + validación + MPU/MMU** por tier |
| Cultura | “Funciona en demo” | **Verify en HW** antes de documentar soporte |

**Ligero sin sacrificar seguridad:** en chips con MPU (F407, F769) el sandbox
hardware refuerza la frontera kernel/user. En **lite** (F103, futuro AVR) no
hay MPU; la defensa es validación estricta en syscalls, buffers acotados y
**anchor** (*fail-safe*) que devuelve el sistema a un estado conocido.

Un solo codebase abraza desde Cortex-M3 sin FPU hasta (futuro) x86/x64 con
MMU — ver [`ARCHITECTURE.md`](ARCHITECTURE.md) — sin `if cfg!(target_arch)`
esparcido en cada función del core.

---

## Personalidades

Rugus adopta **forma distinta según el chip**. No es un único binario
universal; es un **mismo diseño** con backends y políticas adaptadas.

### `lite` — F103, futuro AVR

- **Sin MPU** en hardware.
- Validación de punteros y syscalls acotados; aislamiento cooperativo.
- Memoria muy limitada (p. ej. 20 KiB SRAM en Blue Pill).
- **Anchor** (`sys_failsafe`): estado seguro ante config corrupta o app
  defectuosa.
- Referencia HW: [`boards/stm32f103c8-bluepill.md`](boards/stm32f103c8-bluepill.md).

### `full` — F407, F769

- **MPU** con regiones priv/user; apps en sandbox userland.
- Red, TLS, crypto en crates dedicados (`rugus-net`, `rugus-tls`).
- Scheduler cooperativo con heap acotado; dominios con report de fault.
- Placas: F407G-DISC1, F769I-DISCO.

### `power` (futuro) — x86/x64

- **MMU**, procesos con espacios de direcciones separados.
- Objetivo a largo plazo: **OS de propósito general** con gráficos, shell
  on-device, file system rico — ver G5–G∞ en [`ROADMAP.md`](ROADMAP.md).
- Misma filosofía de capas; el TCB sigue siendo pequeño y auditable.

---

## Capa 0 — invariantes del kernel

La **Capa 0** es el sótano. Estas propiedades no se negocian en review:

1. **Syscalls fijos y versionados** — tabla cerrada; cambios de ABI documentados.
2. **Buffers acotados** — nada de alloc ilimitado en rutas críticas; pools
   estáticos (`heapless`) en kernel.
3. **Sin parse de SD en IRQ** — lectura/parsing de `.rfn` ocurre en contexto
   de tarea o servicio userland, nunca en handler de interrupción.
4. **Watchdog** — el kernel mantiene el WDT alimentado; apps no pueden
   desactivarlo sin política explícita.
5. **Defaults seguros** — GPIO, red y config arrancan en estado conservador
   hasta `config_commit` válido.
6. **Kernel serio** — soporta carga en extremos (red, varios módulos) pero
   el binario del núcleo permanece **pequeño y revisable**.

Detalle verificable en [`INVARIANTS.md`](INVARIANTS.md).

---

## Léxico `rugus-cli` v1

Comandos de usuario con nombres evocadores; cada uno mapea a una operación
del sistema (syscall o servicio). La capa CLI es **cosmética** — el kernel
solo ve IDs y argumentos validados.

| Comando CLI | Operación | Descripción breve |
|-------------|-----------|-------------------|
| `cosmos` | `sys_info` | Información del sistema (versión, personalidad, uptime) |
| `orbit` | `help` | Ayuda local de comandos |
| `ecosystem` | `sys_status` | Estado global (tareas, memoria, red) |
| `moor` | `gpio_bind` | Asociar pin a función/línea lógica |
| `pulso` | `gpio_read` | Leer nivel GPIO |
| `spark` | `gpio_write high` | Escribir GPIO alto |
| `mute` | `gpio_write low` | Escribir GPIO bajo |
| `ripple` | `gpio_toggle` | Invertir GPIO |
| `scout` | `bus_scan` | Escanear bus (I2C/SPI/UART módulos) |
| `sonar` | `module_read` | Leer de módulo conectado |
| `schema` | `config_get` | Obtener valor de config RFN |
| `scribe` | `config_set` | Establecer valor (staging) |
| `seal` | `config_commit` | Persistir config validada |
| `nest` | `module_list` | Listar módulos detectados |
| `hatch` | `app_reload` | Recargar app `.afr` |
| `coil` | `task_list` | Listar tareas del scheduler |
| `anchor` | `sys_failsafe` | Entrar en modo fail-safe |
| `ward` | `wdt` | Estado / petición watchdog |

---

## Formatos de configuración y aplicaciones

### `.rfn` — Rugus Field Notation

Formato **texto** para configuración en SD (campos, pines, módulos, políticas).
Parseado fuera del kernel; el resultado validado llega al sistema vía
`config_set` / `config_commit`.

### `.afr` — Application for Rugus

Paquete de **aplicación** Rugus (metadatos + bytecode o blob firmado según
política futura). Carga con `app_reload`; ejecución en dominio userland en
personalidad `full`.

### v2 (futuro)

- `.rfnz` / `.afrz` — variantes **binarias** con compresión.
- Investigación abierta sobre ratio tamaño/parseo en MCUs pequeños; sin
  compromiso de fecha.

---

## Pinout de referencia — Blue Pill (appliance)

Asignación prevista para el tier **lite** como placa appliance (CLI + módulos
+ SD). Ajustar en RFN por board concreto.

| Función | Periférico | Pines | Notas |
|---------|------------|-------|-------|
| `rugus-cli` | USART1 | PA9 TX, PA10 RX | Consola principal |
| Módulos (LoRa, HM-10 BLE) | USART2 | PA2 TX, PA3 RX | Bus de módulos serie |
| Expansión | USART3 | PB10 TX, PB11 RX | UART adicional |
| Tarjeta SD | SPI1 | PA4 NSS, PA5 SCK, PA6 MISO, PA7 MOSI | Config RFN / logs |
| Sensores | I2C1 | PB6 SCL, PB7 SDA | Ambiente, IMU, etc. |
| Debug (solo desarrollo) | RTT | SWD | `defmt`; no en producción |

Documentación de placa: [`boards/stm32f103c8-bluepill.md`](boards/stm32f103c8-bluepill.md).

---

## Rugus como proyecto open source

Rugus es **software libre**. El fundamento debe permanecer **revisable y
minimalista**: cualquier auditor o contribuidor puede leer `rugus-core` y
`rugus-arch-*` en una tarde y entender el TCB.

### Terremotos — amenazas que el fundamento debe sobrevivir

| “Terremoto” | Ejemplo | Respuesta en Capa 0 |
|-------------|---------|---------------------|
| Entrada hostil | Comandos malformados por UART/BLE | Validación en syscall; rechazo acotado |
| SD corrupta | `.rfn` truncado o malicioso | Parser en userland; `anchor` si commit inválido |
| Atacante en enlace BLE | Inyección de frames | Stack fuera del TCB; timeouts y límites |
| App buggy | Fault en userland | MPU (full) o kill de tarea; kernel sigue vivo |

Los **pisos superiores** pueden ser reemplazados o extendidos por la comunidad
(nueva GUI, otro CLI, apps en `.afr`) **sin reescribir el kernel**. Ese es el
contrato open source de Rugus: código público, **disciplina arquitectónica
privada** en el sentido de estándares del proyecto, no de repos cerrados.

---

## Documentos relacionados

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — crates, trait `Arch`, RTOS vs OS
- [`INVARIANTS.md`](INVARIANTS.md) — reglas verificables post-merge
- [`SYSCALL_ABI.md`](SYSCALL_ABI.md) — ABI de syscalls
- [`ROADMAP.md`](ROADMAP.md) — hitos G0–G∞ y Rugus lite
- [`boards/README.md`](boards/README.md) — placas soportadas y planificadas
