#!/usr/bin/env bash
#
# ota_check.sh — stage OTA fixtures for controlled bench testing.
#
# Usage:
#   ota_check.sh stage <root> <version> <bin> [--sha-of <file>] [--oversize <bytes>]
#
# Writes, under <root>:
#   version               <- "<version>"
#   <version>.bin         <- <bin>, or <bytes> zero bytes with --oversize
#   <version>.bin.sha256  <- sha256 of the .bin, or of <file> with --sha-of
#
# Scenarios:
#   normal      stage <root> 2 2.bin
#   oversize    stage <root> 2 2.bin --oversize 2000000
#   bit-flip    stage <root> 2 2.bin --sha-of 1.bin     # served bin != its hash
#   downgrade   stage <root> 1 1.bin                    # while running v2
#   same-ver    stage <root> <current> <current>.bin
#
# Serve the root with tests/bench/ota_http_server.py and trigger the device via
# `mosquitto_pub -t skylight/reset -m reset`.
set -euo pipefail

if [ "${1:-}" != stage ]; then
  echo "usage: $0 stage <root> <version> <bin> [--sha-of <file>] [--oversize <bytes>]" >&2
  exit 2
fi
shift

root=${1:?missing <root>}
version=${2:?missing <version>}
bin=${3:?missing <bin>}
shift 3

sha_of=""
oversize=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --sha-of) sha_of=${2:?missing value for --sha-of}; shift 2 ;;
    --oversize) oversize=${2:?missing value for --oversize}; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$root"
printf '%s\n' "$version" > "$root/version"

if [ "$oversize" -gt 0 ]; then
  head -c "$oversize" /dev/zero > "$root/$version.bin"
else
  cp "$bin" "$root/$version.bin"
fi

sha256sum "${sha_of:-$root/$version.bin}" | awk '{print $1}' > "$root/$version.bin.sha256"
printf 'staged version=%s oversize=%s sha_of=%s\n' "$version" "$oversize" "${sha_of:-self}"
