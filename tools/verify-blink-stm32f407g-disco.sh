#!/usr/bin/env bash
# Automated build, flash, and RTT verification for blink-stm32f407g-disco.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$ROOT/examples/blink-stm32f407g-disco"
TARGET="thumbv7em-none-eabihf"
ELF="$ROOT/target/$TARGET/release/blink-stm32f407g-disco"
CHIP="STM32F407VG"
LOG="${RUGUS_RTT_LOG:-/tmp/rugus-rtt-verify-f407.log}"
RTT_TIMEOUT="${RUGUS_RTT_TIMEOUT:-25}"
# When several ST-Links are connected, set e.g. PROBE_RS_PROBE=0483:3752:...
PROBE_ARGS=()
if [[ -n "${PROBE_RS_PROBE:-}" ]]; then
  PROBE_ARGS=(--probe "$PROBE_RS_PROBE")
fi

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

echo "=== Rugus verify: blink-stm32f407g-disco ==="
echo "Root: $ROOT"
echo "Log:  $LOG"
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

if grep -qiE '168 MHz|SYSCLK 168' "$LOG"; then
  record_pass "RTT: SYSCLK 168 MHz"
else
  record_fail "RTT: SYSCLK 168 MHz"
fi

if grep -qiE 'LD4|PD12|toggling' "$LOG"; then
  record_pass "RTT: LD4 configured"
else
  record_fail "RTT: LD4 configured"
fi

if grep -qiE 'HardFault|panic|Exception.*halt|defmt version found, but no' "$LOG"; then
  record_fail "fault or defmt/link error in log"
else
  record_pass "no fault / defmt error detected"
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
if [[ $fail -gt 0 ]]; then
  echo "Tip: if RTT is empty but LED blinks, try RUGUS_RTT_TIMEOUT=30 or rebuild from examples/blink-stm32f407g-disco/."
  exit 1
fi
exit 0
