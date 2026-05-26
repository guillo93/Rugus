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

- `PHY link up`
- `IPv4 ready`
- `TCP established`
- `TLS session open`
- `HTTP response: HTTP/1.1 ...`
- `https-get complete`

## Verify script

```bash
./tools/verify-https-get-stm32f769-disco.sh
./tools/verify-eth-link-stm32f769-disco.sh   # regression: step 1 still OK
```

## Stack notes

- **SDRAM heap** (512 KiB) for TLS + smoltcp; falls back to internal RAM if SDRAM fails.
- TLS record buffers (~20 KiB) live on the main stack frame inside the HTTPS block.
- **ETH IRQ** wakes the poll loop (`take_eth_irq_pending` + `WFI`) during TCP/TLS.
