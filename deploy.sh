#!/usr/bin/env bash
#
# deploy.sh — build, publish and trigger a skylights OTA release.
#
# The published version always comes from the repository-root VERSION file;
# this script never increments it. Bump VERSION in an explicit commit first.
#
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
VERSION_FILE="$SCRIPT_DIR/VERSION"

TARGET="xtensa-esp32-none-elf"
APP_PACKAGE="skylights-app"
ELF="$SCRIPT_DIR/target/$TARGET/release/$APP_PACKAGE"
# OTA_SLOT_SIZE from skylights-core/src/ota/partition.rs: 1856 KiB.
MAX_IMAGE_BYTES=$((1856 * 1024))

DRY_RUN=0
STAGE_DIR=""

usage() {
  cat <<'USAGE'
Usage: deploy.sh [--dry-run] [--help]

Build the skylights ESP32 app image, publish it for OTA, then trigger the
device to update. The version comes from the repository-root VERSION file.

Options:
  --dry-run  Build, extract and hash locally (so the printed hash is real),
             then print the remote commands instead of running them. Nothing
             is copied, published or triggered.
  --help     Show this help and exit.

Env file:
  Values may come from the environment or from an env file. If it exists,
  $SCRIPT_DIR/.env.deploy is sourced (override with DEPLOY_ENV_FILE); exported
  variables take precedence over values from the file.

  DEPLOY_ENV_FILE  path to the env file (default: $SCRIPT_DIR/.env.deploy)

Required environment:
  DEPLOY_PUBLISH_HOST  ssh/scp target, e.g. user@host
  DEPLOY_PUBLISH_PATH  remote base directory (nginx firmware root)
  DEPLOY_PROJECT       remote project sub-directory (OTA_PROJECT)
  DEPLOY_MQTT_BROKER   MQTT broker host for the reset trigger

  Optional environment:
  DEPLOY_MQTT_PORT       MQTT broker port (default: mosquitto_pub's own)
  DEPLOY_MQTT_USER       MQTT username (never echoed)
  DEPLOY_MQTT_PASSWORD   MQTT password (never echoed)
  DEPLOY_TRIGGER_TOPIC   reset topic (default: skylight/reset)
  DEPLOY_TRIGGER_PAYLOAD reset payload (default: reset)
USAGE
}

die() {
  printf 'deploy.sh: error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [ -n "$STAGE_DIR" ] && [ -d "$STAGE_DIR" ]; then
    rm -rf -- "$STAGE_DIR"
  fi
}

# Wrap a value in single quotes for a remote POSIX shell, escaping any embedded
# single quote, so an env-supplied path cannot break out of its remote command.
shell_quote() {
  local s=$1
  s=${s//\'/\'\\\'\'}
  printf "'%s'" "$s"
}

run() {
  local arg
  local redact_next=0
  local display=()
  for arg in "$@"; do
    if [ "$redact_next" -eq 1 ]; then
      display+=("****")
      redact_next=0
    else
      display+=("$arg")
      if [ "$arg" = "-P" ] || [ "$arg" = "-u" ]; then
        redact_next=1
      fi
    fi
  done
  printf '+ ' >&2
  printf '%q ' "${display[@]}" >&2
  printf '\n' >&2
  if [ "$DRY_RUN" -eq 0 ]; then
    "$@"
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --help|-h) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
  shift
done

trap cleanup EXIT

# 1. Resolve the version from the repo-root VERSION file.
[ -f "$VERSION_FILE" ] || die "VERSION file not found: $VERSION_FILE"
VERSION="$(tr -d '[:space:]' < "$VERSION_FILE")"
case "$VERSION" in
  ''|*[!0-9]*) die "VERSION must match ^[0-9]+$: $VERSION_FILE" ;;
esac

# 2. Load an optional local env file, then validate the environment before
#    touching the network (or building).

# The default is a gitignored, never-committed file next to this script.
ENV_FILE="${DEPLOY_ENV_FILE:-$SCRIPT_DIR/.env.deploy}"
if [ -f "$ENV_FILE" ]; then
  # The caller's environment wins over the file: snapshot the exported DEPLOY_*
  # variables, source the file, then re-export the snapshot.
  env_names=()
  env_values=()
  while IFS= read -r name; do
    case "$name" in
      DEPLOY_*) env_names+=("$name"); env_values+=("${!name}") ;;
    esac
  done < <(compgen -e)
  printf 'deploy.sh: loading environment from %s\n' "$ENV_FILE" >&2
  set -a
  # shellcheck disable=SC1090,SC1091
  . "$ENV_FILE"
  set +a
  for i in "${!env_names[@]}"; do
    export "${env_names[i]}=${env_values[i]}"
  done
elif [ -n "${DEPLOY_ENV_FILE:-}" ]; then
  die "DEPLOY_ENV_FILE is set to '$DEPLOY_ENV_FILE' but that file does not exist"
fi

for var in DEPLOY_PUBLISH_HOST DEPLOY_PUBLISH_PATH DEPLOY_PROJECT DEPLOY_MQTT_BROKER; do
  if [ -z "${!var:-}" ]; then
    die "required environment variable $var is not set"
  fi
done

# Reject values that would escape the intended remote directory or be parsed as
# ssh/scp options. DEPLOY_PROJECT must be a single path segment matching the
# device's own project charset ([A-Za-z0-9._-]); a `/` or `..` would publish to
# a path the firmware can never read and could steer the remote prune outside
# the firmware root. A leading `-` on the host would be read by OpenSSH as an
# option rather than a hostname.
case "$DEPLOY_PROJECT" in
  ''|.|..|*[!A-Za-z0-9._-]*|*..*) die "DEPLOY_PROJECT must be a single safe directory name (letters, digits, '.', '_', '-'): $DEPLOY_PROJECT" ;;
esac
case "$DEPLOY_PUBLISH_HOST" in
  -*) die "DEPLOY_PUBLISH_HOST must not begin with a hyphen: $DEPLOY_PUBLISH_HOST" ;;
esac

DEPLOY_TRIGGER_TOPIC="${DEPLOY_TRIGGER_TOPIC:-skylight/reset}"
DEPLOY_TRIGGER_PAYLOAD="${DEPLOY_TRIGGER_PAYLOAD:-reset}"

REMOTE_DIR="$DEPLOY_PUBLISH_PATH/$DEPLOY_PROJECT"
REMOTE_BIN="$REMOTE_DIR/$VERSION.bin"
REMOTE_SHA="$REMOTE_DIR/$VERSION.bin.sha256"

# 3. The firmware reads OTA credentials from a gitignored secrets.rs. Fail
#    before the (slow) build if the operator has not created it yet.
SECRETS_FILE="$SCRIPT_DIR/$APP_PACKAGE/src/secrets.rs"
[ -f "$SECRETS_FILE" ] || die "$SECRETS_FILE not found; copy $APP_PACKAGE/src/secrets.example.rs to $APP_PACKAGE/src/secrets.rs and fill in the OTA/WiFi credentials"

# 4. Build the release image.
printf 'Building %s (%s, release)...\n' "$APP_PACKAGE" "$TARGET"
cargo build --release --package "$APP_PACKAGE" --target "$TARGET" -Zbuild-std=core,alloc

# 5. Extract the ESP32 app image (with ESP-IDF image header) and check it
#    against the slot budget. rust-objcopy would emit a headerless raw ELF that
#    the ESP32 bootloader/OTA validator rejects; espflash save-image produces a
#    valid app image (first byte 0xE9). This is a bare-metal esp-hal app, not
#    ESP-IDF, so it carries no app-descriptor section; --ignore-app-descriptor
#    tells espflash that is intentional rather than an error.
[ -f "$ELF" ] || die "build artifact not found: $ELF"
STAGE_DIR="$(mktemp -d)"
BIN="$STAGE_DIR/$VERSION.bin"
SHA="$STAGE_DIR/$VERSION.bin.sha256"
espflash save-image --chip esp32 --ignore-app-descriptor "$ELF" "$BIN"

size="$(wc -c < "$BIN")"
[ "$size" -gt 0 ] || die "extracted image is empty: $BIN"
[ "$size" -le "$MAX_IMAGE_BYTES" ] || die "extracted image is $size bytes, over the $MAX_IMAGE_BYTES-byte OTA-slot budget"
printf 'image size: %s bytes, budget %s\n' "$size" "$MAX_IMAGE_BYTES"

# 6. Hash the image; write only the lowercase hex digest (no filename).
HASH="$(sha256sum "$BIN" | awk '{ print $1 }')"
printf '%s\n' "$HASH" > "$SHA"

printf '\nrelease plan\n'
printf '  version:         %s\n' "$VERSION"
printf '  local image:     %s\n' "$BIN"
printf '  local checksum:  %s\n' "$SHA"
printf '  sha256:          %s\n' "$HASH"
printf '  remote dir:      %s:%s\n' "$DEPLOY_PUBLISH_HOST" "$REMOTE_DIR"
printf '  remote image:    %s:%s\n' "$DEPLOY_PUBLISH_HOST" "$REMOTE_BIN"
printf '  remote checksum: %s:%s\n' "$DEPLOY_PUBLISH_HOST" "$REMOTE_SHA"

# 7. Ensure the remote directory exists and is web-traversable. The published
#    files must be readable by the web server regardless of the SSH user's
#    umask, or nginx returns 403 to the OTA client.
run ssh "$DEPLOY_PUBLISH_HOST" "mkdir -p -- $(shell_quote "$REMOTE_DIR") && chmod 755 $(shell_quote "$REMOTE_DIR")"

# 8. Copy the image, then its checksum, and make both web-readable.
run scp "$BIN" "$DEPLOY_PUBLISH_HOST:$REMOTE_BIN"
run scp "$SHA" "$DEPLOY_PUBLISH_HOST:$REMOTE_SHA"
run ssh "$DEPLOY_PUBLISH_HOST" "chmod 644 $(shell_quote "$REMOTE_BIN") $(shell_quote "$REMOTE_SHA")"

# 9. Publish `version` LAST: it is the commit point the device polls. Write a
#    temp file and rename it into place so a concurrent poll never observes a
#    truncated (empty) version.
run ssh "$DEPLOY_PUBLISH_HOST" "printf '%s\n' $(shell_quote "$VERSION") > $(shell_quote "$REMOTE_DIR/version.tmp") && chmod 644 $(shell_quote "$REMOTE_DIR/version.tmp") && mv -f $(shell_quote "$REMOTE_DIR/version.tmp") $(shell_quote "$REMOTE_DIR/version")"

# 10. Prune older pairs on the remote (best-effort): keep the current version
#     and the largest version strictly below it, delete every other
#     .bin/.bin.sha256 pair. A failed prune only means stale extra pairs remain,
#     so it warns and continues; the device trigger below still runs. This step
#     requires `bash` on the remote host (the `bash -s` body uses `$((10#$n))`);
#     a bash-less remote simply warns and skips cleanup.
if ! run ssh "$DEPLOY_PUBLISH_HOST" bash -s -- "$VERSION" "$(shell_quote "$REMOTE_DIR")" <<'REMOTE_PRUNE'
set -eu
version=$((10#$1))
dir="$2"
prev=""
cd "$dir"
for f in *.bin; do
  [ -e "$f" ] || continue
  base="${f%.bin}"
  case "$base" in
    ''|*[!0-9]*) continue ;;
  esac
  n=$((10#$base))
  if [ "$n" -lt "$version" ]; then
    if [ -z "$prev" ] || [ "$n" -gt "$prev" ]; then
      prev="$n"
    fi
  fi
done
for f in *.bin; do
  [ -e "$f" ] || continue
  base="${f%.bin}"
  case "$base" in
    ''|*[!0-9]*) continue ;;
  esac
  n=$((10#$base))
  if [ "$n" -eq "$version" ]; then
    continue
  fi
  if [ -n "$prev" ] && [ "$n" -eq "$prev" ]; then
    continue
  fi
  rm -f -- "$base.bin" "$base.bin.sha256"
done
REMOTE_PRUNE
then
  printf 'deploy.sh: warning: remote prune failed; stale release pairs may remain.\n' >&2
fi

# 11. Trigger the device reset over MQTT.
mqtt_args=(-h "$DEPLOY_MQTT_BROKER" -t "$DEPLOY_TRIGGER_TOPIC" -m "$DEPLOY_TRIGGER_PAYLOAD")
if [ -n "${DEPLOY_MQTT_PORT:-}" ]; then
  mqtt_args+=(-p "$DEPLOY_MQTT_PORT")
fi
if [ -n "${DEPLOY_MQTT_USER:-}" ]; then
  mqtt_args+=(-u "$DEPLOY_MQTT_USER")
fi
if [ -n "${DEPLOY_MQTT_PASSWORD:-}" ]; then
  mqtt_args+=(-P "$DEPLOY_MQTT_PASSWORD")
fi
run mosquitto_pub "${mqtt_args[@]}"

# 12. Summary.
if [ "$DRY_RUN" -eq 1 ]; then
  printf '\nDry run complete: version %s built and hashed locally; nothing was published.\n' "$VERSION"
else
  printf '\nPublished version %s\n' "$VERSION"
  printf '  %s\n' "$REMOTE_BIN"
  printf '  %s\n' "$REMOTE_SHA"
  printf '  %s\n' "$REMOTE_DIR/version"
fi
