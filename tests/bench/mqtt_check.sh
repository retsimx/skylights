#!/usr/bin/env bash
#
# mqtt_check.sh — legacy-parity MQTT checks for the skylights firmware.
#
# Requires: mosquitto_pub, mosquitto_sub.
# Env:
#   MQTT_HOST (required)   broker host
#   MQTT_PORT (default 1883)
#   MQTT_USER, MQTT_PASSWORD (optional)
#   SAMPLE_TIMEOUT (default 70)  seconds to wait for a moved window to settle
#
# Covers checklist E2-E7, E9. E1/E6/E8 are asserted from the serial log.
# Exit status is non-zero if any automated check fails.
set -euo pipefail

: "${MQTT_HOST:?set MQTT_HOST to the broker host}"
MQTT_PORT="${MQTT_PORT:-1883}"
SAMPLE_TIMEOUT="${SAMPLE_TIMEOUT:-70}"

MQTT=( -h "$MQTT_HOST" -p "$MQTT_PORT" -q 0 )
if [ -n "${MQTT_USER:-}" ]; then MQTT+=( -u "$MQTT_USER" ); fi
if [ -n "${MQTT_PASSWORD:-}" ]; then MQTT+=( -P "$MQTT_PASSWORD" ); fi

CAPTURE="$(mktemp)"
SUB_PID=""
PASS=0
FAIL=0

cleanup() {
  if [ -n "$SUB_PID" ]; then kill "$SUB_PID" 2>/dev/null || true; fi
  rm -f "$CAPTURE"
}
trap cleanup EXIT

mosquitto_sub "${MQTT[@]}" -t 'skylight/#' -v >"$CAPTURE" 2>&1 &
SUB_PID=$!
sleep 1

pub() { mosquitto_pub "${MQTT[@]}" -t "$1" -m "$2"; }
pass() { printf 'PASS %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL %s\n' "$1"; FAIL=$((FAIL + 1)); }

expect() { # desc regex timeout_s
  local desc=$1 regex=$2 timeout=${3:-5} _
  for _ in $(seq 1 "$((timeout * 5))"); do
    if grep -qE -- "$regex" "$CAPTURE"; then pass "$desc"; return 0; fi
    sleep 0.2
  done
  fail "$desc (no match for /$regex/)"
  return 1
}

echo "== E4: get/response =="
pub 'skylight/get' '{"index":1}'
expect "E4 get/response is compact JSON" 'skylight/get/response \{"index":1,"percentage":[0-9]+\}' 5

echo "== E3/E7: set + state on movement =="
pub 'skylight/set' '{"index":1,"percentage":50}'
expect "E7 state published while moving" 'skylight/state .*"index":1,"percentage":[0-9]+,"moving":true' 5
expect "E3 window settles at 50%" 'skylight/state .*"index":1,"percentage":50,"moving":false' "$SAMPLE_TIMEOUT"

echo "== E2/E9: heartbeat + version field =="
expect "E2/E9 state carries version" 'skylight/state .*"version":[0-9]+' "$SAMPLE_TIMEOUT"

echo "== E5: stop =="
pub 'skylight/set' '{"index":2,"percentage":100}'
sleep 1
pub 'skylight/stop' '{"index":2}'
expect "E5 stop halts window 2" 'skylight/state .*"index":2,"percentage":[0-9]+,"moving":false' 15

echo "== E6: reset =="
pub 'skylight/reset' 'reset'
echo "(E6 is confirmed from the serial log: 'MQTT: OTA reset requested')"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
