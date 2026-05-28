#!/usr/bin/env bash
# Automated build, flash, and RTT verification for blink-stm32f103c8-bluepill.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$ROOT/examples/blink-stm32f103c8-bluepill"
TARGET="thumbv7m-none-eabi"
ELF="$ROOT/target/$TARGET/release/blink-stm32f103c8-bluepill"
CHIP="STM32F103C8"
LOG="${RUGUS_RTT_LOG:-/tmp/rugus-rtt-verify-f103.log}"
RTT_TIMEOUT="${RUGUS_RTT_TIMEOUT:-25}"
# Default: external ST-Link V2 (Blue Pill). Override when multiple probes are connected.
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

echo "=== Rugus verify: blink-stm32f103c8-bluepill ==="
echo "Root: $ROOT"
echo "Log:  $LOG"
echo "Probe: $PROBE_RS_PROBE"
echo

cd "$ROOT"
run_check "build (workspace release)" \
  cargo build --workspace --release --target "$TARGET"

run_check "build (blink + defmt link)" \
  bash -c "cd \"$EXAMPLE\" && cargo build --release"

run_check "clippy (workspace, -D warnings)" \
  cargo clippy --workspace --all-targets --target "$TARGET" -- -D warnings

if readelf -S "$ELF" 2>/dev/null | grep -q '\.defmt '; then
  record_pass "ELF has .defmt section"
else
  record_fail "ELF missing .defmt section"
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

if grep -qiE '8 MHz|SYSCLK 8' "$LOG"; then
  record_pass "RTT: SYSCLK 8 MHz"
else
  record_fail "RTT: SYSCLK 8 MHz"
fi

if grep -qiE 'PC13|active low|toggling' "$LOG"; then
  record_pass "RTT: PC13 configured"
else
  record_fail "RTT: PC13 configured"
fi

if grep -qiE 'HardFault|panic|Exception.*halt|defmt version found, but no|chipid 0x000|JtagGetIdcodeError' "$LOG"; then
  record_fail "fault, defmt/link error, or probe connect failure in log"
else
  record_pass "no fault / defmt / probe error detected"
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
if [[ $fail -gt 0 ]]; then
  echo "Tip: external ST-Link wiring — SWDIO→DIO, SWCLK→CLK, GND→GND, 3.3V→3.3V."
  echo "     BOOT0 jumper to GND for normal flash; try PROBE_RS_PROBE from \`probe-rs list\`."
  exit 1
fi
exit 0
