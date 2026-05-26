#!/usr/bin/env bash
# Automated build, flash, and RTT verification for dual-blink-stm32f407g-disco.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$ROOT/examples/dual-blink-stm32f407g-disco"
TARGET="thumbv7em-none-eabihf"
ELF="$ROOT/target/$TARGET/release/dual-blink-stm32f407g-disco"
CHIP="STM32F407VG"
LOG="${RUGUS_RTT_LOG:-/tmp/rugus-rtt-dual-verify-f407.log}"
RTT_TIMEOUT="${RUGUS_RTT_TIMEOUT:-30}"
# Default F407 onboard ST-Link when F769 is also connected:
PROBE_RS_PROBE="${PROBE_RS_PROBE:-0483:3752:066EFF575353667267172509}"
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

echo "=== Rugus verify: dual-blink-stm32f407g-disco ==="
echo "Root: $ROOT"
echo "Log:  $LOG"
echo "Probe: $PROBE_RS_PROBE"
echo

cd "$ROOT"
run_check "build (workspace release)" \
  cargo build --workspace --release --target "$TARGET"

run_check "build (dual-blink + defmt link)" \
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
  if grep -q 'Finished in' "$LOG" || grep -qiE 'INFO|task' "$LOG"; then
    record_pass "flash/run completed"
  else
    record_fail "flash/run (no success indicators)"
  fi
else
  record_fail "probe-rs exit code $probe_exit"
fi

if grep -qiE '168 MHz|SYSCLK 168' "$LOG"; then
  record_pass "RTT: SYSCLK 168 MHz"
else
  record_fail "RTT: SYSCLK 168 MHz"
fi

if grep -qiE 'heap on internal SRAM|heap alloc smoke test OK' "$LOG"; then
  record_pass "RTT: heap on internal SRAM"
else
  record_fail "RTT: heap on internal SRAM"
fi

if grep -qiE 'task A.*started' "$LOG"; then
  record_pass "RTT: task A started"
else
  record_fail "RTT: task A started"
fi

if grep -qiE 'task B.*started' "$LOG"; then
  record_pass "RTT: task B started"
else
  record_fail "RTT: task B started"
fi

if grep -qiE 'HardFault|panic|Exception.*halt|defmt version found, but no' "$LOG"; then
  record_fail "fault or defmt/link error in log"
else
  record_pass "no fault / defmt error detected"
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
if [[ $fail -gt 0 ]]; then
  exit 1
fi
exit 0
