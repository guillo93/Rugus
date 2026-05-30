#!/usr/bin/env bash
# Host-side HM-10/HM-20 factory reset + optional pre-provision (bench).
#
# Uses python3 + pyserial (same stack as verify-appliance UART checks).
# Does NOT flash Rugus — only talks to the BLE module UART (USB-TTL or Rugus
# USART2 passthrough if the module is wired and KEY is at 3.3 V).
#
# Usage:
#   ./tools/provision-hm20.sh -p /dev/ttyUSB0
#   ./tools/provision-hm20.sh -p /dev/ttyUSB0 --provision
#   ./tools/provision-hm20.sh -h
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT=""
PROVISION=0
BAUD_PROBE=(9600 57600 115200)
NAME="RUGUS"
# Datasheet HM-20: AT+BAUD[P] con 3=9600 (fábrica), 7=115200.
BAUD_CODE=7

usage() {
  cat <<'EOF'
provision-hm20.sh — factory reset HM-10/HM-20 (host bench)

  -p, --port PATH     Serial device (required), e.g. /dev/ttyUSB0
  --provision         After reset, set AT+NAMERUGUS and AT+BAUD7 (115200)
  -h, --help          This help

Requires: python3, pyserial (pip install pyserial)

Wiring (direct to module): USB-TTL TX→RX, RX→TX, GND, 3.3V, KEY→3.3V.
Disconnect phone BLE pairing before AT+RENEW.

Exit codes:
  0  success (AT probe + RENEW + RESET + verify OK)
  1  usage / missing deps
  2  serial open or AT probe failed at all bauds
  3  AT+RENEW or AT+RESET failed
  4  post-reset verify failed
  5  --provision failed
EOF
}

die() {
  echo "[ERROR] $*" >&2
  exit "${2:-1}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -p|--port)
      PORT="${2:-}"
      shift 2
      ;;
    --provision)
      PROVISION=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1 (try -h)" 1
      ;;
  esac
done

[[ -n "$PORT" ]] || die "-p/--port is required (e.g. -p /dev/ttyUSB0)" 1

if ! command -v python3 >/dev/null 2>&1; then
  die "python3 not found" 1
fi

if ! python3 -c 'import serial' 2>/dev/null; then
  die "pyserial missing — run: pip install pyserial" 1
fi

echo "=== Rugus HM-20 host provision ==="
echo "Port: $PORT"
echo "Provision after reset: $([[ $PROVISION -eq 1 ]] && echo yes || echo no)"
echo

export HM20_PORT="$PORT"
export HM20_PROVISION="$PROVISION"
export HM20_NAME="$NAME"
export HM20_BAUD_CODE="$BAUD_CODE"

python3 <<'PY'
import os
import sys
import time

try:
    import serial
except ImportError:
    print("pyserial not installed", file=sys.stderr)
    sys.exit(1)

PORT = os.environ["HM20_PORT"]
PROVISION = os.environ.get("HM20_PROVISION", "0") == "1"
NAME = os.environ.get("HM20_NAME", "RUGUS")
BAUD_CODE = int(os.environ.get("HM20_BAUD_CODE", "4"))
BAUDS = [9600, 57600, 115200]
READ_TIMEOUT = 2.0
POST_RESET_WAIT = 1.5


def log(msg: str) -> None:
    print(f"[hm20] {msg}")


def read_all(ser: serial.Serial, idle_s: float = 0.35) -> str:
    time.sleep(idle_s)
    chunks: list[str] = []
    while ser.in_waiting:
        chunks.append(
            ser.read(ser.in_waiting).decode("utf-8", errors="replace")
        )
        time.sleep(0.05)
    return "".join(chunks)


def send_at(ser: serial.Serial, cmd: str, expect_ok: bool = True) -> tuple[bool, str]:
    # Datasheet HM-20: comandos AT SIN terminador (\r\n corrompe nombre/parsing).
    ser.reset_input_buffer()
    ser.write(cmd.encode("ascii"))
    text = read_all(ser)
    ok = "OK" in text or "OK+" in text
    if expect_ok and not ok:
        log(f"FAIL {cmd.strip()!r} → {text!r}")
        return False, text
    log(f"OK   {cmd.strip()!r} → {text.strip()!r}")
    return True, text


def probe_baud(baud: int) -> tuple[serial.Serial | None, str]:
    try:
        ser = serial.Serial(PORT, baud, timeout=READ_TIMEOUT)
    except serial.SerialException as e:
        log(f"cannot open {PORT} @ {baud}: {e}")
        return None, ""
    time.sleep(0.2)
    ok, text = send_at(ser, "AT", expect_ok=True)
    if ok:
        return ser, text
    ser.close()
    return None, text


def main() -> int:
    ser: serial.Serial | None = None
    for baud in BAUDS:
        log(f"probing {baud} baud…")
        ser, _ = probe_baud(baud)
        if ser is not None:
            log(f"AT responded at {baud}")
            break
    if ser is None:
        log("no AT response at 9600, 57600 or 115200 — check KEY→3.3V, TX/RX swap, power")
        return 2

    ok, _ = send_at(ser, "AT+RENEW")
    if not ok:
        ser.close()
        return 3
    ok, _ = send_at(ser, "AT+RESET")
    if not ok:
        ser.close()
        return 3

    ser.close()
    log(f"waiting {POST_RESET_WAIT}s after module reset…")
    time.sleep(POST_RESET_WAIT)

    ser, _ = probe_baud(9600)
    if ser is None:
        log("post-reset AT failed at 9600 (expected factory baud)")
        return 4

    send_at(ser, "AT+NAME?", expect_ok=False)
    send_at(ser, "AT+BAUD?", expect_ok=False)

    if PROVISION:
        ok, _ = send_at(ser, f"AT+NAME{NAME}")
        if not ok:
            ser.close()
            return 5
        ok, _ = send_at(ser, f"AT+BAUD{BAUD_CODE}")
        if not ok:
            ser.close()
            return 5
        ser.close()
        time.sleep(0.3)
        ser, _ = probe_baud(115200)
        if ser is None:
            log("verify at 115200 after AT+BAUD7 failed")
            return 5
        send_at(ser, "AT+NAME?", expect_ok=False)
        send_at(ser, "AT+BAUD?", expect_ok=False)
        log(f"provisioned name={NAME} baud=115200 (BAUD{BAUD_CODE})")
    else:
        send_at(ser, "AT")
        log("factory reset complete — flash Rugus appliance; adopts module baud (9600 default)")

    ser.close()
    return 0


sys.exit(main())
PY
rc=$?
if [[ $rc -ne 0 ]]; then
  echo
  echo "=== FAILED (exit $rc) ==="
  exit "$rc"
fi
echo
echo "=== SUCCESS ==="
exit 0
