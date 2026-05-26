# G4 — Informe matutino (sesión nocturna 2026-05-26)

PR: https://github.com/guillo93/Rugus/pull/24  
Rama: `feat/g4-eth-smoltcp`  
Placa: STM32F769I-DISCO, probe `0483:374b:066EFF524853837267102836`, LAN `192.168.0.x`

---

## 1. CI (GitHub Actions)

| Check | Estado |
|-------|--------|
| rustfmt | ✅ PASS |
| cargo doc (`RUSTDOCFLAGS=-D warnings`) | ✅ PASS |
| build dev / release | ✅ PASS |
| clippy `-D warnings` | ✅ PASS |

**Último run verde:** `26433207575` (commit `10fe98f` en remoto).

**Commits locales pendientes de push** (esta sesión): corrección MPU ETH + `.eth_dma` @ `0x20078000`, `enable_with_eth_dma()`, `service_dma()`, documentación.

Tras `git push`, ejecutar:

```bash
gh pr checks 24
```

Hasta que los cinco jobs estén en verde de nuevo.

---

## 2. Verificación en hardware

### eth-link — `verify-eth-link-stm32f769-disco.sh`

| Resultado | Detalle |
|-----------|---------|
| **9/9 PASS** | Build, clippy, defmt, flash, SYSCLK 216 MHz, PHY link up, IPv4 `192.168.0.50`, sin fault |

RTT tras arreglo MPU: sin `HardFault` / `MemManage` (antes fallaba en `0x2007c004` por región MPU mal alineada y `AP=111` reservado).

### Contadores RX/TX y ping desde PC

| Prueba | Resultado |
|--------|-----------|
| RTT `ETH rx=` / `tx=` | **Siempre 0** durante 45–90 s de ejecución |
| `ping 192.168.0.50` desde Fedora (`192.168.0.112`) | **Fallo** — ARP `FAILED` / `INCOMPLETE`, destino no alcanzable |
| Servidor OpenSSL en `192.168.0.112:8443` | ✅ OK en host (`curl -k https://127.0.0.1:8443/`) |

### https-get — `verify-https-get-stm32f769-disco.sh`

| Resultado | Detalle |
|-----------|---------|
| **9/13 PASS** | Hasta IPv4 + sin fault |
| **4 FAIL** | TCP established, TLS, HTTP, complete — `tcp connect failed: timeout` |

Causa coherente: **capa 2 placa ↔ PC no intercambia tramas** (mismo síntoma que ping), no un fallo del servidor TLS en el host.

---

## 3. Correcciones aplicadas esta noche

1. **`configure_eth_mpu()`** — ahora se llama vía `cache::enable_with_eth_dma()` en ejemplos G4 (antes nunca se invocaba).
2. **MPU** — `AP=011` (RW completo); región **16 KiB @ `0x20078000`** (alineada; ST usaba `0x2007C000` sin alineación MPU válida).
3. **Linker** — `.eth_dma` movido a `0x20078000` en `memory.x` de `eth-link` y `https-get`.
4. **`EthernetDMA::service_dma()`** — limpia `RBUS` y dispara poll demand RX; usado tras link-up y en el bucle de `eth-link`.
5. **Flash** — `probe-rs download` necesario cuando `probe-rs run` no reprogramaba (ELF antiguo en flash).

---

## 4. Cómo fusionar el PR (usuario)

1. `git pull` en `feat/g4-eth-smoltcp` y revisar commits de la noche.
2. Confirmar `gh pr checks 24` — todo verde.
3. Revisar diff G4 (`rugus-hal-stm32f7::eth`, `rugus-net`, `rugus-tls`, ejemplos).
4. **Merge** en GitHub (no se hizo merge automático en esta sesión).

---

## 5. Pruebas ping recomendadas (Windows / Linux)

### Linux (misma LAN que la placa)

```bash
# PC en 192.168.0.x (ej. 192.168.0.112)
ip -4 addr show
ping -c 5 192.168.0.50
ip neigh show 192.168.0.50   # debe mostrar REACHABLE, no FAILED
```

Con firmware `eth-link` corriendo (RTT activo):

```bash
export PROBE_RS_PROBE=0483:374b:066EFF524853837267102836
cd examples/eth-link-stm32f769-disco
cargo build --release
probe-rs download --chip STM32F769NIHx target/thumbv7em-none-eabihf/release/eth-link-stm32f769-disco
probe-rs run --chip STM32F769NIHx --log-format full --rtt-scan-memory \
  target/thumbv7em-none-eabihf/release/eth-link-stm32f769-disco
# En otra terminal, mientras corre:
ping 192.168.0.50
```

Buscar en RTT: `ETH rx=` **> 0** al hacer ping.

### Windows

1. Cable Ethernet PC ↔ switch/router **mismo segmento** que la F769 (CN3).
2. IP estática PC: `192.168.0.112`, máscara `255.255.255.0`, puerta de enlace `192.168.0.1` (opcional).
3. `ping 192.168.0.50` en cmd o PowerShell.
4. Si falla: probar **cable directo** PC ↔ CN3 (sin router) y IP PC `192.168.0.112/24`.

### HTTPS (cuando ping/ARP funcione)

```bash
openssl s_server -accept 8443 -www -cert /tmp/rugus-cert.pem -key /tmp/rugus-key.pem -servername rugus-test
./tools/verify-https-get-stm32f769-disco.sh
```

---

## 6. Qué funcionó / qué requiere acción del usuario

| ✅ Funcionó | ⚠️ Requiere tu acción |
|------------|----------------------|
| CI verde en remoto (`10fe98f`) | `git push` de commits MPU + docs |
| eth-link 9/9 script | Confirmar L2: cable en **CN3 F769** (no puerto de otra placa) |
| PHY link + IPv4 estático en RTT | Probar **cable directo** PC↔placa si el router aísla clientes |
| Servidor TLS en host | Tras L2 OK: repetir verify https **13/13** |
| Sin MemManage tras fix MPU | Si `rx=0` persiste: captura `tcpdump -i enp1s0 arp` mientras corre eth-link |

### Hipótesis L2 (router vs cable)

- Placa y PC en `192.168.0.0/24` pero **sin tráfico Ethernet observado** (`rx=0`).
- Posibles causas: puerto LAN incorrecto, switch con aislamiento AP/client, VLAN, o necesidad de enlace **punto a punto** sin router.
- El firmware responde ARP/ICMP vía smoltcp **solo si el DMA RX recibe frames**; hoy el cuello de botella parece físico/topología, no la pila IP configurada.

---

## 7. ROADMAP / CHANGELOG

- `docs/ROADMAP.md` — G4 ya marcado cerrado en documentación.
- `CHANGELOG.md` — sección `[0.5.0]` describe G4; añadir nota MPU en próximo push si se desea.

---

*Generado por agente nocturno Rugus G4. No se fusionó el PR.*
