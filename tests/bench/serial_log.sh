#!/usr/bin/env bash
#
# serial_log.sh — capture ESP32 UART (defmt) to a timestamped log file.
#
# Usage: serial_log.sh [LOGFILE]
# Env:   BENCH_SERIAL (default /dev/ttyUSB0), BENCH_CHIP (default esp32)
#
# The firmware emits defmt-framed logs (esp-println `defmt-espflash`), which
# espflash decodes with `--log-format defmt`. Ctrl-C stops the capture.
set -euo pipefail

SERIAL="${BENCH_SERIAL:-/dev/ttyUSB0}"
CHIP="${BENCH_CHIP:-esp32}"
LOG="${1:-serial-$(date -u +%Y%m%d-%H%M%S).log}"

printf 'capturing %s -> %s (Ctrl-C to stop)\n' "$SERIAL" "$LOG" >&2
exec espflash monitor \
  --chip "$CHIP" \
  --port "$SERIAL" \
  --non-interactive \
  --log-format defmt 2>&1 | tee -a "$LOG"
