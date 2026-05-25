#!/usr/bin/env bash
# Automated build, flash, and RTT verification for blink-stm32f769-disco.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$ROOT/examples/blink-stm32f769-disco"
TARGET="thumbv7em-none-eabihf"
ELF="$ROOT/target/$TARGET/release/blink-stm32f769-disco"
CHIP="STM32F769NIHx"
LOG="${RUGUS_RTT_LOG:-/tmp/rugus-rtt-verify.log}"
RTT_TIMEOUT="${RUGUS_RTT_TIMEOUT:-25}"

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

echo "=== Rugus verify: blink-stm32f769-disco ==="
echo "Root: $ROOT"
echo "Log:  $LOG"
echo

cd "$ROOT"
run_check "build (workspace release)" \
  cargo build --workspace --release --target "$TARGET"

# Must run from example dir so .cargo/config.toml (defmt.x, link.x) is applied.
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
timeout "$RTT_TIMEOUT" probe-rs run --chip "$CHIP" --log-format full --rtt-scan-memory "$ELF" \
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

if grep -qiE '216 MHz|SYSCLK 216' "$LOG"; then
  record_pass "RTT: SYSCLK 216 MHz"
else
  record_fail "RTT: SYSCLK 216 MHz"
fi

if grep -qiE 'LD1|PJ13|toggling' "$LOG"; then
  record_pass "RTT: LD1 configured"
else
  record_fail "RTT: LD1 configured"
fi

if grep -qiE 'HardFault|panic|Exception.*halt|defmt version found, but no' "$LOG"; then
  record_fail "fault or defmt/link error in log"
else
  record_pass "no fault / defmt error detected"
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
if [[ $fail -gt 0 ]]; then
  echo "Tip: if RTT is empty but LED blinks, try RUGUS_RTT_TIMEOUT=30 or rebuild from examples/blink-stm32f769-disco/."
  exit 1
fi
exit 0
