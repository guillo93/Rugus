#!/usr/bin/env bash
# Automated build, flash, and RTT verification for app-sandbox-stm32f407g-disco (G3).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$ROOT/examples/app-sandbox-stm32f407g-disco"
TARGET="thumbv7em-none-eabihf"
ELF="$ROOT/target/$TARGET/release/app-sandbox-stm32f407g-disco"
CHIP="STM32F407VG"
LOG="${RUGUS_RTT_LOG:-/tmp/rugus-rtt-sandbox-verify-f407.log}"
RTT_TIMEOUT="${RUGUS_RTT_TIMEOUT:-45}"
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

echo "=== Rugus verify: app-sandbox-stm32f407g-disco (G3) ==="
echo "Root: $ROOT"
echo "Log:  $LOG"
echo "Probe: $PROBE_RS_PROBE"
echo

cd "$ROOT"
run_check "build (workspace release)" \
  cargo build --workspace --release --target "$TARGET"

run_check "build (app-sandbox + defmt link)" \
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
  if grep -qiE 'INFO|kernel|sandbox' "$LOG"; then
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

if grep -qiE 'heap on internal SRAM' "$LOG"; then
  record_pass "RTT: heap on internal SRAM"
else
  record_fail "RTT: heap on internal SRAM"
fi

if grep -qiE 'kernel task.*started' "$LOG"; then
  record_pass "RTT: kernel task started"
else
  record_fail "RTT: kernel task started"
fi

if grep -qiE 'MemManage.*domain=App|fault MemManage.*App|killing task' "$LOG"; then
  record_pass "RTT: MemManage fault reported (domain App)"
else
  record_fail "RTT: MemManage fault + domain App"
fi

if grep -qiE 'killing task' "$LOG"; then
  record_pass "RTT: kernel killed faulting task"
else
  record_fail "RTT: task kill policy"
fi

if grep -qiE 'kernel toggle LD4' "$LOG"; then
  record_pass "RTT: kernel continued after fault"
else
  record_fail "RTT: kernel continued after fault"
fi

if grep -qiE 'HardFault.*handler mode|defmt version found, but no|global panic' "$LOG"; then
  record_fail "global panic / handler HardFault in log"
else
  record_pass "no global panic detected"
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
if [[ $fail -gt 0 ]]; then
  exit 1
fi
exit 0
