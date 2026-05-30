# Ecosistema Rugus y capacidades instalables `.eco`

Documento de diseño sobre el **ecosistema** Rugus: el concepto paraguas que une
el estado vivo del sistema, el catálogo de capacidades instalables y lo que
está efectivamente **plantado** en una placa. Complementa
[`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md) (metáfora del edificio) y
[`RUGUS-LITE-APPLIANCE.md`](RUGUS-LITE-APPLIANCE.md) (firmware lite).

> Documento de **diseño**, no de implementación. No introduce cambios en crates
> de firmware ni de host; fija terminología y arquitectura para los PRs que sí
> los toquen.

---

## Terminología bloqueada

Para evitar ambigüedad, estos términos se usan **exactamente** así en todo el
proyecto:

| Término | Significado |
|---------|-------------|
| **ecosystem** | Concepto paraguas. También el comando CLI `ecosystem` (estado vivo del sistema), el **registry/catálogo** de capacidades instalables y el conjunto de capacidades **plantadas** en la placa. |
| **eco** | Una capacidad instalable individual: un sensor, un módulo de comunicaciones, una regla. |
| **`.eco`** | El archivo que distribuye una capacidad `eco`. |

### Metáfora narrativa — la espora

El ecosistema se cuenta como un jardín: el usuario **planta una espora** y esta
**germina en una capacidad** viva dentro del sistema. La *espora* es solo
imaginería; el artefacto y el término técnico son siempre **`.eco`** / **eco**.

Esta imagen se alinea con la [metáfora del edificio](RUGUS-KERNEL-VISION.md):
el sótano (kernel serio) no germina nada por sí mismo; las capacidades crecen en
los pisos medios (servicios/userland) y la experiencia vive en la cima
(`rush`, `rugus-cli`).

---

## 1. Qué es un `.eco`

Un **`.eco`** es una **capacidad reutilizable** definida principalmente por
**configuración declarativa**, con **lógica mínima opcional**. El objetivo de
diseño es contundente:

> **El usuario no escribe código nativo para instalar una capacidad.**

El kernel **interpreta y valida** el descriptor `.eco`; **nunca ejecuta código
nativo arbitrario** en el tier **lite**. Un `.eco` describe *qué* es la
capacidad (bus, identificación, registros, lecturas, reglas) y el kernel sabe
*cómo* hablar con ese tipo de dispositivo de forma acotada y segura.

Propiedades de un `.eco`:

- **Declarativo primero** — texto legible, estilo `.rfn`.
- **Sin compilación por parte del usuario** en el caso declarativo puro.
- **Acotado** — buffers, tamaños y operaciones tienen límites explícitos.
- **Verificable** — pensado para llevar firma (ver §6) y validarse antes de
  germinar.
- **Reemplazable** — una capacidad puede desinstalarse o sustituirse sin tocar
  el TCB.

Lo que un `.eco` **no** es:

- No es un binario que el kernel cargue y ejecute en lite.
- No es un atajo para meter drivers en `rugus-core`.
- No es lógica de presentación: la cosmética vive en `rush`/`rugus-cli`.

---

## 2. Dos sabores de `.eco`

Un `.eco` puede ser **declarativo** o **híbrido**.

| Sabor | Contenido | Ejemplo | Código nativo |
|-------|-----------|---------|---------------|
| **Declarativo** | Solo configuración: bus, dirección, registros, lecturas. | Descriptor de sensor I2C (BME280). | Ninguno. El kernel interpreta. |
| **Híbrido** | Config + **lógica mínima**: reglas `if/then`, *timers*, umbrales. Puede referenciar un `.afr`. | Sensor + regla "si temp > X, enciende GPIO". | Solo reglas acotadas evaluadas por el motor; nada arbitrario en lite. |

### Declarativo (solo config)

Describe un periférico y cómo leerlo. El kernel ya sabe manejar el bus
(I2C/SPI/UART/1-Wire); el `.eco` aporta los parámetros. No hay lógica
ejecutable.

### Híbrido (config + lógica mínima)

Añade un bloque de **reglas** acotadas: comparaciones simples, *timers* y
acciones sobre recursos ya validados (GPIO, otro módulo). Para automatización
más rica, el `.eco` **referencia un `.afr`** (ver
[`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md) §formatos), que corre en
dominio userland — nunca en el sótano.

> Regla dura: incluso en el sabor híbrido, en **lite** el motor evalúa reglas
> *interpretadas* y acotadas. No se ejecuta código máquina entregado por el
> usuario.

---

## 3. Relación con `.rfn` y `.afr`

Los tres formatos son complementarios y ya conviven en la visión del kernel:

| Formato | Rol | Ámbito | Ejecución |
|---------|-----|--------|-----------|
| **`.rfn`** | Configuración de placa: *pinout*, buses, políticas. | El sistema base (qué hardware hay). | Parseado en userland; validado vía `config_commit`. |
| **`.afr`** | Aplicación / automatización. | Lógica de usuario rica. | Dominio userland (`full`), `app_reload` / `hatch`. |
| **`.eco`** | Capacidad **instalable**: sensor, comms, regla. | Lo que se "planta" sobre el sistema base. | Interpretado/validado por el kernel; lógica mínima acotada o delega en `.afr`. |

Lectura corta:

- **`.rfn`** dice *qué pines y buses existen*.
- **`.eco`** dice *qué capacidad vive sobre esos buses* y cómo identificarla.
- **`.afr`** dice *qué hace la aplicación* con todo lo anterior.

Un `.eco` puede **asumir** un `.rfn` (necesita que el bus I2C esté declarado) y
puede **referenciar** un `.afr` (para automatización compleja).

---

## 4. Ejemplo `.eco`

### 4.1 Declarativo — sensor BME280 (I2C)

```eco
# bme280.eco — capacidad declarativa
eco.name      = bme280
eco.kind      = sensor
eco.flavor    = declarativo

bus           = i2c
i2c.id        = 0x76

# Identificación: confirmar chip antes de germinar
whoami.reg    = 0xD0
whoami.value  = 0x60

# Lectura de temperatura (registro de datos crudos)
read.temp.reg = 0xFA
read.temp.len = 3
```

Flujo conceptual: el kernel **escanea** I2C (ver §5), encuentra `0x76`, lee el
registro `whoami` `0xD0`, confirma `0x60` y **germina** la capacidad
`bme280`. Sin código nativo del usuario.

### 4.2 Híbrido — sensor + regla

```eco
# bme280-aviso.eco — capacidad híbrida (config + regla mínima)
eco.name      = bme280-aviso
eco.kind      = sensor
eco.flavor    = hibrido

bus           = i2c
i2c.id        = 0x76
whoami.reg    = 0xD0
whoami.value  = 0x60
read.temp.reg = 0xFA
read.temp.len = 3

# Regla mínima interpretada por el motor (acotada, fail-safe)
rule.if       = temp > 40
rule.then     = gpio_write C13 high
rule.period   = 2s

# Automatización rica opcional: delega en una app .afr userland
app.ref       = clima.afr
```

La regla `if/then` la evalúa el **motor de reglas** del kernel de forma acotada
(comparación simple + acción sobre un recurso ya validado). La parte compleja
(`clima.afr`) corre en userland.

---

## 5. Detección e identificación

Antes de germinar, una capacidad debe **detectarse** e **identificarse**. La
estrategia depende del bus.

| Bus | Detección | Identificación | Notas |
|-----|-----------|----------------|-------|
| **I2C** | Escaneo de direcciones (`scout`) | Confirmación `whoami` (registro + valor) | Barato; base del descubrimiento de sensores. |
| **1-Wire** | Enumeración de dispositivos | **ROM family code** | Identifica tipo por código de familia. |
| **SPI** | Declarado en `.eco` | Declarado (no hay scan universal) | Selección por CS; el `.eco` describe el protocolo. |
| **UART** | Declarado en `.eco` | Declarado (`IDENTIFY` para módulos Rugus) | Módulos serie (BLE, LoRa) por USART2. |

El comando `scout` (ver §10) realiza el escaneo I2C ("explorar el terreno"). La
confirmación `whoami` evita germinar una capacidad sobre un chip equivocado: es
parte de la **validación** previa a la instalación.

> El parseo y el escaneo ocurren en contexto de tarea/servicio, **nunca en
> handler de IRQ** (invariante de Capa 0).

---

## 6. Registry firmado

El **registry** es el catálogo de capacidades disponibles: descriptores `.eco`
y, para módulos complejos, **crates nativos opcionales** (drivers que sí
requieren código, fuera del flujo declarativo lite).

| Elemento del registry | Contenido | Firma |
|-----------------------|-----------|-------|
| Descriptor `.eco` | Config declarativa/híbrida | **Ed25519** |
| Crate nativo (módulo complejo) | Driver nativo para tiers con carga real | **Ed25519** |
| Metadatos de catálogo | Nombre, versión, tier objetivo, dependencias | Firmado en conjunto |

Propiedades:

- **Firmas Ed25519** sobre cada artefacto del catálogo.
- **Anclado a G6** — la verificación de firmas se integra con *verified boot* y
  **OTA** (ver [`ROADMAP.md`](ROADMAP.md), hito G6). Una capacidad no germina si
  la firma no valida.
- **`rugus-cli` (host)** descarga del registry, verifica e **instala/planta**
  hacia la placa. El kernel solo acepta artefactos validados.

La cadena de confianza es la misma que protege el arranque y las
actualizaciones: nada se "planta" sin firma verificada.

---

## 7. Instalación por tier

Cómo se instala una capacidad depende de la personalidad del hardware
(ver [`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md) §personalidades).

| Tier | Hardware | `.eco` declarativo | Módulo nativo |
|------|----------|--------------------|---------------|
| **lite** | F103, futuro AVR (sin MMU) | **Hot-install**: descriptor a flash interna o SD; germina sin reflash. | Requiere **rebuild + reflash** del firmware. |
| **full** | F407, F769 (con MPU) | Hot-install validado | **Slots firmados** en sandbox userland (MPU). |
| **power** | x86/x64 (con MMU, futuro) | Hot-install validado | **Carga dinámica nativa real** (procesos, espacios separados). |

Idea clave:

- En **lite**, un `.eco` **declarativo** se instala en caliente porque el kernel
  lo *interpreta*; un driver **nativo** nuevo exige recompilar y reflashear.
- En **full**, los módulos nativos viven en **slots firmados** aislados por MPU.
- En **power**, hay **carga dinámica nativa** de verdad, con aislamiento por MMU.

El descriptor declarativo es el camino universal; el código nativo escala su
mecanismo de carga con las capacidades del chip.

---

## 8. ¿SD obligatoria? NO

La tarjeta SD **no es obligatoria**. Habilita instalación en caliente y
catálogos grandes, pero el sistema funciona sin ella.

| Escenario | Almacén de `.eco` | Catálogo | Hot-install |
|-----------|-------------------|----------|-------------|
| **Sin SD** | Flash interna | Pocas capacidades (espacio limitado) | Limitado: depende del flash disponible. |
| **Con SD** | `/sd/ecos/` | Catálogo grande | Sí: plantar/quitar sin reflash. |

- **Sin SD**: unos pocos `.eco` caben en flash interna; suficiente para un
  appliance con función fija.
- **Con SD**: directorio `/sd/ecos/` aloja un catálogo amplio y permite
  **hot-install** cómodo. Es el escenario recomendado para experimentar con
  muchas capacidades.

La SD es **opcional pero potenciadora**: desbloquea hot-install y catálogo
grande sin volverse un requisito del kernel.

---

## 9. Reglas del kernel serio

El ecosistema **no debilita** el sótano. El kernel mantiene sus invariantes
(ver [`INVARIANTS.md`](INVARIANTS.md) y
[`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md) §Capa 0):

- **Motor que valida** — todo `.eco` se valida antes de germinar (esquema,
  rangos, identificación `whoami`/family code).
- **Buffers acotados** — lecturas, reglas y descriptores tienen tamaños
  límite; nada de alloc ilimitado en rutas críticas.
- **Fail-safe (`anchor`)** — ante descriptor corrupto o regla inválida, el
  sistema cae a estado seguro, no a `panic` global.
- **Sin código arbitrario en lite** — el motor solo interpreta reglas acotadas;
  no ejecuta máquina entregada por el usuario.
- **Firmas verificadas** — sin firma válida no hay germinación (§6).

> El **"wow"** y la gestión cómoda (descubrir catálogo, instalar bonito, ver
> estado) viven en **`rugus-cli` (host)**, **no** en el kernel. El sótano sigue
> pequeño, revisable y antisísmico.

---

## 10. CLI

El ecosistema se opera con comandos ya presentes en el léxico `rush`
(ver [`RUGUS-LITE-APPLIANCE.md`](RUGUS-LITE-APPLIANCE.md)). Recordatorio de
nombres:

- **Shell on-device (firmware): `rush`.**
- **Herramienta de host (escritorio): `rugus-cli`.**

| Comando | Rol en el ecosistema |
|---------|----------------------|
| `scout` | **Detectar** — escanea buses (I2C) para descubrir hardware candidato a germinar. |
| `nest` | **Listar instaladas** — capacidades/módulos `eco` ya plantados. |
| `ecosystem` | **Estado vivo** — estado global del sistema y `eco` plantados (tareas, memoria, módulos, failsafe). |

### Flujo conceptual instalar/plantar

```text
rugus-cli (host)                 rush (firmware)
----------------                 ---------------
1. fetch .eco del registry
2. verifica firma Ed25519
3. envía descriptor a la placa  ─► recibe y valida (.eco)
                                   confirma identidad (scout/whoami)
                                   "planta" → germina capacidad
4. ecosystem / nest             ◄─ reporta estado y eco plantados
```

El host hace la experiencia rica (catálogo, firma, transferencia); el firmware
solo acepta lo validado y reporta estado. El descubrimiento host↔placa usa el
protocolo `IDENTIFY` (ver [`RUGUS-LITE-APPLIANCE.md`](RUGUS-LITE-APPLIANCE.md)).

---

## 11. Roadmap / fases

Orden de construcción propuesto para el ecosistema:

| Fase | Entrega | Por qué primero |
|------|---------|-----------------|
| 1 | **`.eco` del módulo BLE HM-20** (comunicaciones) | Primer `.eco` real; sirve de **plantilla** para el resto. |
| 2 | **`.eco` de sensores** (BME280 y similares) | Capacidades declarativas sobre la plantilla. |
| 3 | **Registry firmado** | Catálogo + firmas Ed25519 + integración OTA/verified boot (G6). |
| 4 | **Carga dinámica nativa (tier power)** | Módulos nativos reales con MMU. |
| Futuro | **tiny-ML como `.eco`/`.rml` especial** | Slot de inferencia ligera; ver [`RUGUS-LITE-ML.md`](RUGUS-LITE-ML.md). |

El **HM-20 BLE** se elige como primer `.eco` real porque ejercita el camino
completo (descriptor + módulo serie por USART2 + `IDENTIFY`) y deja un patrón
reutilizable para sensores y comms posteriores.

### Registry stub (HM-20)

| Campo | Valor |
|-------|-------|
| ID | `hm20-ble` |
| Archivo | [`examples/eco/hm20-ble.eco`](../examples/eco/hm20-ble.eco) |
| Bus | USART2 @ 115200 (PA2/PA3) |
| Driver HAL | `rugus-hal-stm32f1::hm20` |
| Estado | Instalable declarativo (sin firma Ed25519 aún) |

La inferencia tiny-ML se prevé
como un `.eco` especial (formato `.rml`), encajado en el mismo flujo de
instalación/firma sin ensanchar el TCB.

---

## Documentos relacionados

- [`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md) — metáfora del edificio, capas, personalidades
- [`RUGUS-LITE-APPLIANCE.md`](RUGUS-LITE-APPLIANCE.md) — firmware lite, léxico `rush`, `IDENTIFY`
- [`RUGUS-LITE-ML.md`](RUGUS-LITE-ML.md) — inferencia ligera (futuro `.rml`)
- [`ROADMAP.md`](ROADMAP.md) — hitos G0–G∞ (incl. G6 verified boot/OTA)
- [`INVARIANTS.md`](INVARIANTS.md) — reglas verificables del kernel
