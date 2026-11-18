# Architecture

## Purpose and constraints

The firmware drives three Velux skylights by pulsing an OEM remote's button
pad, accepts absolute position commands over MQTT, tracks position open-loop in
integer milliseconds, and updates itself through a dual-slot OTA with a
30-second trial boot. It is bare metal on an ESP32-WROOM: no OS, no `std`, one
static heap, embassy async tasks.

Two hard constraints shape the code:

1. **The policy crate must stay hardware-free.** `skylights-core` compiles on
   the host and carries all arithmetic, parsing and state machines.
2. **Position is RAM-only and open-loop.** Every reset homes all windows to the
   closed hard seat; correctness depends on calibrated drive times, not a
   sensor.

## Crate layering

```text
┌─────────────────────────────────────────────────────────────┐
│ skylights-app  (xtensa-esp32-none-elf, no_std, no_main)      │
│ GPIO pulses · embassy tasks · Wi-Fi/net · flash OTA driver   │
└───────────────▲─────────────────────────────────────────────┘
                │ depends on (path dependency)
┌───────────────┴─────────────────────────────────────────────┐
│ skylights-core  (host-testable, cfg_attr(not(test), no_std)) │
│ window model · MQTT codec · OTA decisions · Wi-Fi backoff    │
│ version parsing · partition tables · pure storage traits     │
└─────────────────────────────────────────────────────────────┘
```

Dependency direction is one-way: `app → core`. `core` never imports `esp-*`,
`embassy-*`, or app modules. `core`'s only optional dependencies are
`serde`/`serde-json-core` behind the `serde` feature; `sha2` is used for OTA
verification.

## Core module map (`skylights-core/src/`)

| Module | Owns |
|---|---|
| `window.rs` | `Position`, `WindowState`, travel plan/state machine, calibrated affine mapping, pulse constants |
| `mqtt.rs` | Topic constants, payload parsing/formatting, range validation (`Index` 1–3, `Percentage` 0–100) |
| `ota/` | Partition layout and slot selection, `otadata` CRC/sequence handling, update decision and streaming session logic, pure `OtaStorage` trait |
| `wifi.rs` | Join/DHCP/DNS/MQTT timeouts, reconnect backoff, progress watchdogs |
| `net.rs` | Broker URL parsing, link state, backoff |
| `version.rs` | Strict `VERSION` parsing for the build script |

`lib.rs` re-exports the public API; `mod tests` at the bottom of `lib.rs` covers
the position helper that predates `window.rs`.

## App task map (`skylights-app/src/`)

| Task / module | Responsibility |
|---|---|
| `main` | Init `esp_hal`, drive all 9 pins HIGH **first**, probe GPIO safety, heap, banner, TIMG0 (Wi-Fi) / TIMG1 (embassy + watchdog), spawn tasks |
| `net::init` → `net_task`, `connection_task` | embassy-net stack, Wi-Fi join and reconnect supervision |
| `window::window_task` ×3 (`pool_size = 3`) | One interruptible controller per window: receive command, pulse, wait travel, finish or stop |
| `mqtt::mqtt_task` | Subscribe `skylight/+`, parse commands, publish get/state, 60 s heartbeat, signal `STATE_CHANGED` |
| `ota::ota_task` | Wait on `OTA_TRIGGER`, fetch version/sha/image, stream into passive slot, mark trial boot, reset |
| `ota::selftest::self_test_task` | 30 s trial window with an 8 s TIMG1 watchdog; confirm `ESP_OTA_IMG_VALID` or roll back `ESP_OTA_IMG_INVALID`; then runtime supervision |

Task count is fixed at build time by the embassy executor arena; new tasks
need an arena-size review.

## Data flows

### Command ingress

```text
MQTT skylight/set {"index":N,"percentage":P}
  → mqtt_task parse/validate (integer-only, 0..=100)
  → WINDOW_COMMANDS[N-1].send(SetTarget(Position))
  → window_task: positioner.request()
  → actuate Open/Close pulse burst
  → wait plan.travel_ms (interruptible by Stop or a newer SetTarget)
  → Stop pulse (partial) or hard seat (boundary)
  → positioner.finish()/stop()
```

`skylight/stop` sends `Stop`; `skylight/get` replies on
`skylight/get/response`; `skylight/reset` raises `OTA_TRIGGER`.

### Telemetry

`window_task` writes a snapshot to `WINDOW_STATES` and signals
`STATE_CHANGED`. `mqtt_task` publishes `skylight/state` on every signal and
every 60 s heartbeat. The message includes `version` (build-time
`SKYLIGHTS_BUILD_VERSION`), so it is the release-verification source of truth.

### OTA

```text
skylight/reset (or boot)
  → ota_task fetches <OTA_BASE_URL>/<project>/version
  → remote != local ? fetch <v>.bin.sha256, then <v>.bin
  → stream 4 KiB sectors into the passive slot (ota_0 ⇄ ota_1) while hashing
  → SHA-256 match ? write otadata trial entry, reset : leave otadata untouched
  → bootloader starts the trial image
  → self_test_task: gpio_safe + wifi_connected + mqtt_healthy within 30 s
      pass → ESP_OTA_IMG_VALID
      fail → ESP_OTA_IMG_INVALID, reset, bootloader rolls back
```

## Flash layout (`partitions.csv`, 4 MiB)

| Region | Offset | Size | Purpose |
|---|---|---|---|
| `nvs` | `0x009000` | 20 KiB | Non-volatile storage |
| `otadata` | `0x00E000` | 8 KiB | Two 4 KiB ping-pong selection sectors |
| `phy_init` | `0x010000` | 4 KiB | RF calibration |
| `ota_0` | `0x020000` | 1856 KiB | Application slot A |
| `ota_1` | `0x1F0000` | 1856 KiB | Application slot B |

`OTA_SLOT_SIZE`, offsets and CRC/sequence rules live in
`skylights-core/src/ota/partition.rs`. OTA images must keep the ESP-IDF `0xE9`
header (`espflash save-image --ignore-app-descriptor`), not a raw objcopy dump.

## Boot and failure model

1. All nine active-low outputs are configured HIGH before any slow init; a
   floating line could read as a pressed button.
2. `probe_gpio_safe` must see all HIGH. A failure invalidates a trial image.
3. Position is RAM-only, so every boot (cold, watchdog, OTA swap) homes all
   windows closed with a full 71 s hard-seat drive. Homing stays interruptible.
4. The 30 s trial window confirms a new image only after GPIO, Wi-Fi and MQTT
   are healthy. Anything else rolls back.
5. After confirmation, the same watchdog instance supervises runtime.

## Where to change what

| Change | Touch |
|---|---|
| Travel timing/position model | `skylights-core/src/window.rs` + tests; README §3 |
| MQTT schema/topics | `skylights-core/src/mqtt.rs` + tests; README §4 |
| GPIO mapping/pulse driver | `skylights-app/src/window.rs`, `main.rs`; README §2 |
| OTA decision/partition logic | `skylights-core/src/ota/` + tests |
| OTA transport/flash driver | `skylights-app/src/ota/`, `flash.rs` |
| Release/publish flow | `VERSION`, `deploy.sh`, README §5 |
