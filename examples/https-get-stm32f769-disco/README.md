# https-get-stm32f769-disco

Rugus **G4** deliverable: HTTPS GET against a LAN server on STM32F769I-DISCO.

## Network layout

| Role | IPv4 | Notes |
|------|------|-------|
| F769 board | `192.168.0.50/24` | Static (`StaticConfig::home_lan()`) |
| Gateway | `192.168.0.1` | Typical home router |
| HTTPS server (PC) | `192.168.0.112:8443` | `Endpoint::lan_https_server()` |

Connect the board to **CN3** (RJ45) and ensure the server PC is on the same LAN.

## LAN HTTPS test server

The firmware uses SNI / Host **`rugus-test`** and skips certificate verification (lab use).

### Option A — OpenSSL (quick)

On the server PC (`192.168.0.112`):

```bash
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout /tmp/rugus-key.pem -out /tmp/rugus-cert.pem -days 365 \
  -subj "/CN=rugus-test"

openssl s_server -accept 8443 -www \
  -cert /tmp/rugus-cert.pem -key /tmp/rugus-key.pem \
  -servername rugus-test
```

### Option B — Python 3

```bash
python3 - <<'PY'
import ssl, socket
from http.server import HTTPServer, SimpleHTTPRequestHandler

ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain("rugus-cert.pem", "rugus-key.pem")  # generate with openssl above
HTTPServer(("0.0.0.0", 8443), SimpleHTTPRequestHandler).socket = ctx.wrap_socket(
    HTTPServer(("0.0.0.0", 8443), SimpleHTTPRequestHandler).socket, server_side=True
)
print("HTTPS on :8443")
HTTPServer(("0.0.0.0", 8443), SimpleHTTPRequestHandler).serve_forever()
PY
```

Adjust IPs in `crates/rugus-net/src/tcp.rs` (`Endpoint::lan_https_server`) if your LAN differs.

## Build & flash

```bash
export PROBE_RS_PROBE=0483:374b:066EFF524853837267102836  # F769 when F407 also connected
cd examples/https-get-stm32f769-disco
cargo run --release
```

Expected RTT (with server running):

- `PHY link up (autoneg done)`
- `static IPv4 192.168.0.50/24`
- `IPv4 ready`
- `L2 probe window 8 s — try ping 192.168.0.50 from host now` *(use this window to confirm L2 reachability)*
- `TCP connect 192.168.0.112:8443`
- `tcp connect: established`
- `TLS session open`
- `HTTP response: HTTP/1.1 ...`
- `https-get complete`

## Verify scripts

```bash
./tools/verify-https-get-stm32f769-disco.sh
./tools/verify-eth-link-stm32f769-disco.sh   # regression: step 1 must stay 9/9
```

## Stack notes

- **SRAM-only heap** (64 KiB at the linker-provided `RAM` region). FMC/SDRAM is **not** initialized — the current working set (TLS rec buffers 16 KiB + 4 KiB, smoltcp ~10 KiB) fits comfortably in internal SRAM and skipping FMC keeps the GPIO `PG` pin bank untouched. If you need SDRAM later for caches/datasets, restore `fmc::init` in `init_heap`.
- TLS record buffers (~20 KiB) live on the main stack frame inside the HTTPS block.
- **ETH IRQ** wakes the poll loop (`take_eth_irq_pending` + `WFI`) during TCP/TLS.
- **`smoltcp_phy::receive/transmit`** auto-services the DMA (`service_dma()`) on every poll, so the main loop does not need to call it.

## Troubleshooting

If `verify-https-get` returns less than 13/13 with `TCP established` failing:

1. Run `verify-eth-link-stm32f769-disco.sh` first. If it is 9/9 PASS and `ping -c 4 192.168.0.50` succeeds from the host, the HAL and L2 are healthy.
2. Capture the wire with `sudo tcpdump -i <iface> -nne 'ether host 00:80:e1:11:22:33'` while reflashing `https-get`. If the board's ARP/SYN never appear on the wire, see `docs/G4-CLOSE-REPORT.md` for the suspected PHY / cable / switch level causes and the recommended hard-cable-cycle / `firewall-cmd` validation steps.
3. The default MAC `00:80:E1:11:22:33` lives in `crates/rugus-net::DEFAULT_MAC` and `crates/rugus-hal-stm32f7::eth::DEFAULT_MAC`. Override if your LAN already uses this address.
