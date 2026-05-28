#!/usr/bin/env bash
# Automated build, flash, and RTT verification for https-get-stm32f769-disco.
#
# Requires a LAN HTTPS server at 192.168.0.112:8443 (see example README).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE="$ROOT/examples/https-get-stm32f769-disco"
TARGET="thumbv7em-none-eabihf"
ELF="$ROOT/target/$TARGET/release/https-get-stm32f769-disco"
CHIP="STM32F769NIHx"
PROBE="${PROBE_RS_PROBE:-0483:374b:066EFF524853837267102836}"
LOG="${RUGUS_RTT_LOG:-/tmp/rugus-https-get-verify.log}"
RTT_TIMEOUT="${RUGUS_RTT_TIMEOUT:-90}"

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

echo "=== Rugus verify: https-get-stm32f769-disco ==="
echo "Root:  $ROOT"
echo "Probe: $PROBE"
echo "Log:   $LOG"
echo "Note:  LAN HTTPS server required at 192.168.0.112:8443"
echo

cd "$ROOT"
run_check "build (workspace release)" \
  cargo build --workspace --release --target "$TARGET"

run_check "build (https-get + defmt link)" \
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
PROBE_RS_PROBE="$PROBE" timeout "$RTT_TIMEOUT" probe-rs run \
  --connect-under-reset --chip "$CHIP" --log-format full --rtt-scan-memory "$ELF" \
  >"$LOG" 2>&1
probe_exit=$?
set -e

cat "$LOG"

if [[ $probe_exit -eq 0 || $probe_exit -eq 124 || $probe_exit -eq 137 ]]; then
  if grep -q 'Finished in' "$LOG" || grep -qiE 'INFO|PHY link|TLS' "$LOG"; then
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

if grep -qiE 'PHY link up|link up' "$LOG"; then
  record_pass "RTT: PHY link up"
else
  record_fail "RTT: PHY link up"
fi

if grep -qiE 'IPv4 ready|static IPv4 192\.168\.0\.50|IPv4 address 192\.168\.0\.50' "$LOG"; then
  record_pass "RTT: static IPv4 192.168.0.50"
else
  record_fail "RTT: static IPv4 192.168.0.50"
fi

if grep -qiE 'TCP established' "$LOG"; then
  record_pass "RTT: TCP established"
else
  record_fail "RTT: TCP established (is HTTPS server running?)"
fi

if grep -qiE 'TLS session open' "$LOG"; then
  record_pass "RTT: TLS session open"
else
  record_fail "RTT: TLS session open"
fi

if grep -qiE 'HTTP response:.*HTTP/' "$LOG"; then
  record_pass "RTT: HTTP response received"
else
  record_fail "RTT: HTTP response (grep HTTP/)"
fi

if grep -qiE 'https-get complete' "$LOG"; then
  record_pass "RTT: https-get complete"
else
  record_fail "RTT: https-get complete"
fi

if grep -qiE 'HardFault|panic|Exception.*halt|defmt version found, but no' "$LOG"; then
  record_fail "fault or defmt/link error in log"
else
  record_pass "no fault / defmt error detected"
fi

echo
echo "=== Summary: $pass passed, $fail failed ==="
if [[ $fail -gt 0 ]]; then
  echo "Tip: start LAN HTTPS server — see examples/https-get-stm32f769-disco/README.md"
  echo "     Board IP: 192.168.0.50, server: 192.168.0.112:8443"
  exit 1
fi
exit 0
