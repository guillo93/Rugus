# Consolas `rush` — convergencia de flota (F6.4)

Rugus es un **kernel de multipersonalidad**: cada placa compone su personalidad
(lite/full) pero todas exponen la misma consola on-device, `rush`, con un
**léxico universal**: el mismo verbo produce la misma salida en cualquier
miembro de la flota. El cliente host universal es `rugus-cli` (ver
[`RUGUS-CLI-HOST.md`](RUGUS-CLI-HOST.md)).

## Canal gateado

Todo transporte exige **autenticación de canal** challenge-response HMAC-SHA256
antes de aceptar verbos: sin sesión abierta solo pasan `IDENTIFY` (ENQ `0x05` o
la palabra `identify`) y el propio handshake (`knock`/`prove`/`lock`/`enroll`).

```
knock                 → challenge <nonce-hex>
prove <hmac(PSK,n)>   → auth: ok — sesión abierta
lock                  → cierra la sesión
enroll <psk-hex>      → aprovisiona la PSK (una sola vez, fábrica)
```

Cada transporte tiene su `Session` propia: autenticar por serie no abre el
canal de red, y viceversa. La PSK nunca sale al cable ni es legible por los
verbos del ABI (`schema`/`scribe`): vive en un almacén que la consola no sabe
enumerar.

## Flota

| Placa | Tier | Transportes | Almacén de PSK |
|-------|------|-------------|----------------|
| F103 Blue Pill (appliance) | lite | USART1 + BLE HM-10 | última página de flash (1 K en `0x0800FC00`, FPEC F1) |
| F407G-DISC1 | full | USART2 (PA2/PA3 @115200) | sector 11 de flash interna (128 K en `0x080E0000`, `rugus-hal-stm32f4::flash`) |
| F769I-DISCO | full | TCP:7777 (192.168.0.50) + USART2 (PA2/PA3 @115200) | primer subsector de la NOR QSPI (MX25L512) |

Los almacenes sobreviven al reflasheo del firmware: en F407 el `memory.x`
limita FLASH a 896 K (el linker jamás pisa el sector 11; ver también la región
MPU `SECRETS`, necesaria porque la región de flash es write-never); en F769 la
QSPI es un medio aparte.

## Léxico (tier full)

La tabla de hooks compartida vive en `rugus-personality-full` (cada placa solo
inyecta sus `BoardOps`); el parser y la sesión en `rush`.

- `cosmos` — identidad y estado del sistema (placa, tier, arranques, tareas).
- `ecosystem` — salud global (tareas, faults totales, causa del último reset).
- `coil` — tabla de tareas con high-water de pila.
- `scar [clear]` — telemetría persistente de faults.
- `letargo` — energía/ocio (uptime, idle %, IRQs de SysTick, entradas a STOP).
- `pulso`/`spark`/`mute`/`ripple`/`moor` — GPIO de placa.
- `scout`/`sonar` — descubrimiento de buses (donde la placa lo cablea).
- `schema`/`scribe`/`seal` — config RFN staging/commit.
- `nest`/`hatch` — módulos y respawn de tareas.
- `anchor`/`ward`/`sting` — failsafe, watchdog y tarea víctima de prueba.
- `orbit` — ayuda.

## Léxico bespoke retirado (F6.4d)

La consola de operador de F4.5 (`rugus-kernel::console::Console`, comandos
`help`/`ps`/`mem`/`faults`/`respawn`/`reboot`) fue retirada al converger la
flota; del módulo queda solo `RxRing` (transporte SPSC IRQ→supervisor).
Equivalencias: `ps`→`coil`, `mem`→`cosmos`, `faults`→`scar`,
`respawn`→`hatch`, `help`→`orbit`.

## Validación de campo

- F407: USB-TTL externo en PA2/PA3 (el VCP del ST-Link de la DISC1 **no** está
  cableado al target). 115200 8N1.
- F769 red: descubrimiento UDP:9001 (`IDENTIFY`), consola TCP:7777. La placa no
  responde ping (smoltcp sin ICMP): comprobar reachability con `arping`.
- Secuencia de humo: ENQ → `RUGUS;tier=...;chip=...` → `knock`/`prove` →
  `cosmos` → `coil` → `scar` → `letargo`.
