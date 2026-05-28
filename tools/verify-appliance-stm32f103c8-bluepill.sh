#!/usr/bin/env bash
# Automated build, flash, and RTT verification for appliance-stm32f103c8-bluepill.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$ROOT/examples/appliance-stm32f103c8-bluepill"
TARGET="thumbv7m-none-eabi"
ELF="$ROOT/target/$TARGET/release/appliance-stm32f103c8-bluepill"
CHIP="STM32F103C8"
LOG="${RUGUS_RTT_LOG:-/tmp/rugus-rtt-verify-appliance-f103.log}"
RTT_TIMEOUT="${RUGUS_RTT_TIMEOUT:-30}"
PROBE_RS_PROBE="${PROBE_RS_PROBE:-0483:3748:55C3BF6B0648C2875752685117C287}"
PROBE_ARGS=(--probe "$PROBE_RS_PROBE")

pass=0
fail=0

record_pass() {
  echo "[PASS] $1"
  pass=$((pass + 1))
}

record_fail() {
  echo "[FAIL] $1"
  fail=$((fail + 1))
}

run_check() {
  local name="$1"
  shift
  if "$@"; then
    record_pass "$name"
  else
    record_fail "$name"
  fi
}

echo "=== Rugus verify: appliance-stm32f103c8-bluepill (phases 1-6) ==="
echo "Root: $ROOT"
echo "Log:  $LOG"
echo "Probe: $PROBE_RS_PROBE"
echo

cd "$ROOT"
run_check "build (workspace release)" \
  cargo build --workspace --release --target "$TARGET"

run_check "build (appliance release)" \
  cargo build --release --target "$TARGET" -p appliance-stm32f103c8-bluepill

run_check "clippy (workspace, -D warnings)" \
  cargo clippy --workspace --all-targets --target "$TARGET" -- -D warnings

if readelf -S "$ELF" 2>/dev/null | grep -qE '\.defmt(\.end)? '; then
  record_pass "ELF has defmt section"
else
  record_fail "ELF missing defmt section"
fi

echo
echo "=== Flash + RTT (${RTT_TIMEOUT}s) ==="
set +e
timeout "$RTT_TIMEOUT" probe-rs run --chip "$CHIP" "${PROBE_ARGS[@]}" --log-format full --rtt-scan-memory "$ELF" \
  >"$LOG" 2>&1
probe_exit=$?
set -e

cat "$LOG"

if [[ $probe_exit -eq 0 || $probe_exit -eq 124 ]]; then
  if grep -q 'Finished in' "$LOG" || grep -q 'INFO' "$LOG"; then
    record_pass "flash/run completed"
  else
    record_fail "flash/run (no success indicators)"
  fi
else
  record_fail "probe-rs exit code $probe_exit"
fi

if grep -qiE 'appliance ready|appliance F103' "$LOG"; then
  record_pass "RTT: appliance boot"
else
  record_fail "RTT: appliance boot"
fi

if grep -qiE 'cli task|heartbeat task' "$LOG"; then
  record_pass "RTT: scheduler tasks"
else
  record_fail "RTT: scheduler tasks"
fi

if grep -qiE 'appliance ready|services ok' "$LOG"; then
  record_pass "RTT: USART1 CLI init"
else
  record_fail "RTT: USART1 CLI init"
fi

if grep -qiE 'HardFault|panic|Exception.*halt|defmt version found, but no|chipid 0x000|JtagGetIdcodeError' "$LOG"; then
  record_fail "fault, defmt/link error, or probe connect failure in log"
else
  record_pass "no fault / defmt / probe error detected"
fi

# Optional UART cosmos check (USB-TTL on PA9/PA10)
if [[ -n "${RUGUS_UART_PORT:-}" ]]; then
  echo
  echo "=== UART cosmos check on ${RUGUS_UART_PORT} ==="
  if command -v python3 >/dev/null 2>&1; then
    UART_LOG="${RUGUS_UART_LOG:-/tmp/rugus-uart-appliance-f103.log}"
    python3 - "$RUGUS_UART_PORT" "$UART_LOG" <<'PY'
import sys, time, serial
port, log_path = sys.argv[1], sys.argv[2]
lines = []
with serial.Serial(port, 115200, timeout=2) as ser:
    time.sleep(0.5)
    ser.reset_input_buffer()
    ser.write(b"cosmos\r\n")
    time.sleep(1.0)
    while ser.in_waiting:
        lines.append(ser.read(ser.in_waiting).decode("utf-8", errors="replace"))
text = "".join(lines)
open(log_path, "w").write(text)
print(text)
if "Rugus lite appliance" in text or "cosmos" in text.lower() or "personality" in text:
    sys.exit(0)
sys.exit(1)
PY
    if [[ $? -eq 0 ]]; then
      record_pass "UART: cosmos response"
    else
      record_fail "UART: cosmos response"
    fi
  else
    record_fail "UART: python3/pyserial not available"
  fi
else
  echo
  echo "[INFO] Set RUGUS_UART_PORT=/dev/ttyUSB0 for UART cosmos verify"
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
if [[ $fail -gt 0 ]]; then
  echo "Tip: BOOT0→GND; ST-Link SWD; PROBE_RS_PROBE from \`probe-rs list\`."
  echo "     UART CLI: minicom -D /dev/ttyUSB0 -b 115200 → type \`cosmos\`"
  exit 1
fi
exit 0
