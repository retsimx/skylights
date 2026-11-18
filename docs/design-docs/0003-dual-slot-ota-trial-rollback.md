# 0003 — Dual-slot OTA with verify-then-commit and trial rollback

- **Status**: Verified (deployed and confirmed on hardware)
- **Date**: 2026-09 (OTA), 2026-11-18 (release/verification notes)
- **Scope**: `skylights-core/src/ota/`, `skylights-app/src/ota/`, `deploy.sh`

## Context

The controller is mounted at the skylights and is not convenient to reach with
USB. Updates must be remote, must survive a bad image, and must not brick a
window controller that also has physical actuators. The ESP32 has 4 MiB flash
and the ESP-IDF bootloader supports two OTA slots plus `otadata`.

The OTA server is a plain file root: `version`, `<v>.bin`,
`<v>.bin.sha256`. The device fetches it at boot and whenever `skylight/reset`
raises `OTA_TRIGGER`; there is no periodic poll.

## Decision

1. **Symmetric slots.** `ota_0` at `0x020000` and `ota_1` at `0x1F0000`, each
   1856 KiB. `otadata` is two 4 KiB ping-pong sectors with CRC and a sequence
   number so a interrupted write cannot select a torn entry.
2. **Verify, then commit.** Stream the image into the passive slot while
   hashing; only a complete, matching SHA-256 writes a trial entry to
   `otadata`. Any read/byte-count/hash failure leaves `otadata` untouched and
   erases the passive slot's first sector on a hash mismatch.
3. **Trial boot.** The new image boots with `ESP_OTA_IMG_NEW`. A 30 s window
   (`SELF_TEST_WINDOW_MS`) under an 8 s `TIMG1` watchdog requires all three
   health signals: `gpio_safe`, `wifi_connected`, `mqtt_healthy`.
4. **Confirm or roll back.** Pass writes `ESP_OTA_IMG_VALID`; fail writes
   `ESP_OTA_IMG_INVALID` and resets, letting the bootloader return to the
   previous valid slot.
5. **Version inequality triggers.** Any remote integer different from the
   local build version (including a downgrade) triggers an update; equality is
   a no-op.
6. **Publish the commit point last.** `deploy.sh` uploads the image and hash,
   then writes `version` via a temp file + rename, then prunes older pairs
   (keeping current and immediate predecessor), then publishes
   `skylight/reset`.

## Release sequence (learned operationally)

```text
1. merge the firmware change (CI: host tests + xtensa build)
2. bump VERSION in its own commit/PR
3. ./deploy.sh --dry-run     # build, extract, size check, hash; no network
4. ./deploy.sh              # publish + trigger
5. verify skylight/state's "version" == new version
```

`VERSION` is read at build time by `build.rs` and becomes
`SKYLIGHTS_BUILD_VERSION`. `deploy.sh` never increments it.

## Verification

`skylight/state` carries `"version"` (the build version), so it is the
post-update source of truth. `skylight/get` returns only index and percentage.
A quick check:

```sh
set -a; . ./.env.deploy; set +a
mosquitto_sub -h "$DEPLOY_MQTT_BROKER" -p "${DEPLOY_MQTT_PORT:-1883}" \
  -t skylight/state -C 1 -W 70
```

Expect the new `version`, `moving:false`, and windows at 0 % after boot homing.
Uptime beyond 30 s means the trial self-test confirmed rather than rolled back.

## Alternatives rejected

- **Single slot + USB reflash**: no remote recovery, no rollback, requires
  physical access at height.
- **Delta/compressed updates**: marginal bandwidth gain for substantial
  complexity and a new failure mode; the full image is 852 KiB against a
  1856 KiB slot.
- **Confirm on boot without a health window**: would keep a broken image that
  cannot reach MQTT; the trial window is the safety property.
- **Trust the download without a hash**: a truncated or corrupted image would
  be marked valid.

## Consequences

- Images must stay under 1856 KiB; `deploy.sh` fails the build if not.
- Flash writes must be 4 KiB sector-aligned (`SlotFlasher`); arbitrary chunk
  writes shift data on ESP32.
- A rollback leaves the old firmware running; the operator sees the old
  `version` and must diagnose from serial logs.
- The release flow depends on the device being online at trigger time; a
  missed trigger updates on the next boot or next `skylight/reset`.
