# skylights — ESP32 Firmware (Native Rust / Embassy)

Bare-metal Rust (`#![no_std]`) firmware for an **ESP32-WROOM** (`xtensa-esp32-none-elf`) controller driving three Velux skylight windows via OEM remote button pads, reporting telemetry and accepting commands over MQTT, and performing streaming dual-slot OTA updates with automatic rollback.

---

## 1. System Overview & Architecture

### Target & Workspace Layout

The firmware targets the classic **ESP32-WROOM** (`xtensa-esp32-none-elf`) using `esp-hal` (`0.23.1`) and the `embassy` async runtime (`esp-hal-embassy`). The repository is a two-crate Cargo workspace:

| Crate | Purpose |
|---|---|
| `skylights-core` | Pure, hardware-agnostic, `#![no_std]` policy library with zero HAL dependencies (`window`, `mqtt`, `ota`, `net`, `wifi`, `version`). All unit tests run natively on the host. |
| `skylights-app` | ESP32 `#![no_std]` / `#![no_main]` firmware image (`main.rs`, `window.rs`, `mqtt.rs`, `net.rs`, `ota/`, `flash.rs`, `heap.rs`, `secrets.rs`, `build.rs`). |

### Memory & Runtime Task Architecture

- **Static Heap (`skylights-app/src/heap.rs`)**: Registers a **96 KiB** (`98,304` byte) global heap via `esp_alloc::HEAP` backed by `.dram2_uninit` in **DRAM2**. Placing the heap in DRAM2 leaves DRAM0 available for the main stack, Embassy's 16 KiB task arena, and static BSS network/TLS buffers.
- **Timers**: `TIMG0` drives `esp-wifi`; `TIMG1` drives `embassy-time` (`timer0`) and the hardware watchdog (`wdt`).

At boot, `main` spawns the following Embassy tasks:

| Task | Source | Responsibility |
|---|---|---|
| `net_task` | `skylights-app/src/net.rs` | Drives the `embassy-net` DHCPv4 / TCP/IP stack (`StackResources<4>`). |
| `connection_task` (Wi-Fi supervisor) | `skylights-app/src/net.rs` | Manages Wi-Fi station association (`15 s` join / `5 s` leave timeouts), DHCP (`3` attempts x `15 s`), DNS probe (`10 s`), exponential backoff (`1 s` to `30 s`), and link-loss / stale-MQTT recovery. |
| `window_task` (x3 pool) | `skylights-app/src/window.rs` | Runs one interruptible `WindowController` per window (`WINDOW_COUNT = 3`), driven by per-window capacity-4 `WINDOW_COMMANDS` channels. |
| `mqtt_task` | `skylights-app/src/mqtt.rs` | Maintains a `minimq` MQTT session over TCP, subscribes to `skylight/+`, dispatches inbound commands, and publishes `60 s` heartbeats plus state-change telemetry. |
| `ota_task` | `skylights-app/src/ota/mod.rs` | Waits on `OTA_TRIGGER` (raised once at boot and on `skylight/reset`), checks `version`, downloads and SHA-256-verifies `{v}.bin`, writes the passive OTA slot, and resets to trial-boot. |
| `self_test_task` | `skylights-app/src/ota/selftest.rs` | Evaluates the 30 s post-swap trial verification window (`ESP_OTA_IMG_VALID` vs `ESP_OTA_IMG_INVALID` rollback) and then runs the permanent `runtime_watchdog`. |

### Watchdog & 10-Minute Recovery Clocks

The firmware layers hardware and software supervision so wedged states or silent network black-holes self-recover without manual power cycling:

- **TIMG1 Hardware Watchdog (`8 s`)**: `self_test_task` arms `TIMG1` WDT with `WATCHDOG_TIMEOUT_US = 8_000_000` (fed at most once every `1 s`). During normal operation (`runtime_watchdog`), feeds are withheld if MQTT progress goes stale past `NO_IP_REBOOT_MS` (`10` minutes), forcing a hardware reset if the executor wedges or traffic stalls permanently.
- **End-to-End MQTT Progress & Radio Recovery (`10` min = `NO_IP_REBOOT_MS = 600_000 ms`)**:
  - **MQTT Rejoin / Stale Abandonment**: `5` consecutive MQTT connect failures (`MQTT_REJOIN_FAILURES`) raise `WIFI_REJOIN`; an established association with no completed MQTT dispatch for `180 s` (`MQTT_STALE_MS`) is abandoned to force a fresh Wi-Fi join.
  - **Clock A (MQTT Progress Reset)**: Resets the chip via `software_reset()` if no MQTT dispatch completes within `10` minutes (even while Wi-Fi remains associated).
  - **Clock B (`NoIpWatchdog`)**: Resets the chip via `software_reset()` if no association and DHCP lease is recovered within `10` minutes (`STABLE_LEASE_MS = 90_000 ms` minimum lease duration required to refresh the watchdog on link loss).

---

## 2. Hardware & Wiring

### GPIO Pin Assignment

All nine outputs (`skylights-app/src/main.rs`) drive active-low lines wired to the OEM Velux remote button pads:

| Window | Open GPIO | Stop GPIO | Close GPIO |
|---|---|---|---|
| **Window 1** | `GPIO26` | `GPIO25` | `GPIO16` |
| **Window 2** | `GPIO19` | `GPIO5` | `GPIO18` |
| **Window 3** | `GPIO21` | `GPIO22` | `GPIO23` |

### Electrical Characteristics & Boot Safety

- **Active-Low 3.3 V Outputs**: Configured as push-pull `Output` pins with external pull-up resistors to `3.3 V`. `HIGH` (`3.3 V`) is idle/released; pulsing `LOW` (`0 V`) emulates pressing the corresponding button on the OEM remote.
- **Immediate Boot-Time `HIGH` Initialization**: Configuring all 9 pins to `Level::High` is the very first operation in `main` (right after `esp_hal::init`), before heap setup, flash reads, or Wi-Fi initialization. `probe_gpio_safe` then verifies all 9 pins read `HIGH` before moving them into `window_task`; a failed probe fails the OTA self-test (`reason=gpio`).
- **Why Window 1 Uses `GPIO26` and `GPIO25`**: Window 1 intentionally avoids strapping and power-on-reset (POR) pull-down pins `GPIO2` (internal POR pull-down plus DevKit onboard LED to GND) and `GPIO4` (internal POR pull-down). Those pins glitch `LOW` during bootloader startup and caused phantom opens on reset; `GPIO26` and `GPIO25` have no strapping or POR pull-downs.

---

## 3. Actuation & Timing Contract

All actuation timing constants live in `skylights-core/src/window.rs` and are executed by `WindowDriver` / `WindowController` in `skylights-app/src/window.rs`.

### Pulse Sequence & Bus Serialization

- **Two-Pulse Burst + Settle**: Every `Open`, `Close`, or `Stop` actuation emits **2 cycles** of **`100 ms` LOW** (`PULSE_LOW_MS = 100`) / **`800 ms` HIGH** (`PULSE_HIGH_MS = 800`), followed by a **`400 ms` settle** delay (`PULSE_SETTLE_MS = 400`).
  - `100 ms` LOW ensures reliable button detection on the OEM remote pad.
  - `800 ms` HIGH gives the remote time to transmit and reset before the second press.
  - Total sequence duration: `2 * (100 + 800) + 400 = 2,200 ms` (`PULSE_SEQUENCE_MS`).
- **Global Serialization (`ACTUATION_LOCK`)**: A shared async `Mutex` ensures only one window can emit a pulse sequence at a time, preventing RF/remote contention across windows.

### Travel Calibration, Interpolation & Boundaries

- **Calibrated Full Travel**: `TRAVEL_TIME_MS = 71_000` (**71 seconds** for `0% <-> 100%`).
- **Linear Virtual Position (`0..=100`)**: Position is tracked in RAM using integer millisecond arithmetic (`travel_duration_ms = |from - to| * 71_000 / 100`).
- **Intermediate Targets (`1..=99`)**:
  - If `target == current_position`, the request is a no-op.
  - Otherwise, the controller pulses `Open` or `Close`, waits the interpolated `travel_ms`, and emits a trailing **`Stop`** pulse (`stop_after = true`) upon arrival.
- **Boundary Targets (`0` and `100`)**:
  - Driving to `0%` (closed) or `100%` (open) always runs the full **`71,000 ms`** drive **without** a trailing `Stop` pulse (`stop_after = false`), even if the virtual position is already `0%` or `100%`.
  - The window motor's internal end-stop limit switches absorb any over-travel, giving a reliable hard-seat recalibration.
- **Mid-Travel Interruption**:
  - Receiving `Stop` mid-travel halts the timer, pulses `Stop`, and computes the new virtual position via `interrupted_position` (`start +/- (elapsed_ms * 100 / 71_000)`, clamped to `0..=100`).
  - Receiving a new `SetTarget` mid-travel pulses `Stop`, records the interpolated position, and immediately begins driving toward the newest target.
- **Fail-Safe Boot Homing**:
  - Because virtual position is RAM-only, **every reset** (cold boot, watchdog reset, or OTA swap) immediately homes all 3 windows to `0%` (`Position::CLOSED`) via a full `71 s` hard-seat close.
  - Boot homing remains interruptible: any inbound `skylight/set` or `skylight/stop` MQTT command cancels the home and takes over like normal travel.

---

## 4. MQTT API Reference

- **Broker Transport**: IPv4 literal configured in `secrets.rs` (`MQTT_BROKER = "mqtt://<ipv4>[:port]"`, default port `1883`, anonymous session, no DNS dependency).
- **Client ID**: `skylights-<12-hex-mac>` derived from the ESP32 eFuse MAC address.
- **Subscription**: `skylight/+` at **QoS 0** (`session_expiry_interval(0)`, `60 s` keepalive).

### Topics & Payloads

| Topic | Direction | Payload Schema & Behavior |
|---|---|---|
| `skylight/set` | Inbound | `{"index": 1..3, "percentage": 0..100}` — drives window `index` (`1..=3`) to `percentage` (`0..=100`). Out-of-range or malformed JSON payloads are ignored and logged. |
| `skylight/get` | Inbound | `{"index": 1..3}` — queries window `index`'s current virtual position. |
| `skylight/get/response` | Outbound (QoS 0) | `{"index": N, "percentage": M}` — published in response to `skylight/get`. |
| `skylight/stop` | Inbound | `{"index": 1..3}` — halts window `index` immediately, pulsing `Stop` if moving and updating its interpolated position. |
| `skylight/reset` | Inbound | Permissive payload (conventionally `"reset"`, though any payload including empty is accepted) — raises `OTA_TRIGGER` to run an OTA update check. |
| `skylight/state` | Outbound (QoS 0) | Published every **`60 s`** (`HEARTBEAT_SECS`) and immediately on every window state change (`STATE_CHANGED`). |

### `skylight/state` JSON Schema

Field order is normative (`StateMessage` in `skylights-core/src/mqtt.rs`):

```json
{
  "windows": [
    {"index": 1, "percentage": 0, "moving": false},
    {"index": 2, "percentage": 50, "moving": false},
    {"index": 3, "percentage": 100, "moving": true}
  ],
  "version": 8,
  "wifi_rssi": 0,
  "uptime_secs": 120
}
```

- `windows`: array of 3 objects (`index` `1..=3`, `percentage` `0..=100`, `moving` boolean).
- `version`: integer firmware build version from `VERSION` (`SKYLIGHTS_BUILD_VERSION`).
- `wifi_rssi`: signed integer dBm (`0` placeholder when unknown).
- `uptime_secs`: seconds elapsed since boot.

---

## 5. Flashing, OTA & Release Workflow

### Initial / Local USB Flashing

For local development with a USB-UART cable attached, `.cargo/config.toml` configures `espflash flash --monitor` as the Cargo runner:

```sh
. "$HOME/export-esp.sh"
cargo run
```

To build a full factory image (bootloader + partition table + application) and flash at offset `0x0`:

```sh
cargo build --release --package skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc

espflash save-image --chip esp32 --merge --partition-table partitions.csv \
  target/xtensa-esp32-none-elf/release/skylights-app /tmp/skylights-factory.bin

espflash write-bin --chip esp32 --port /dev/ttyUSB0 0x0 /tmp/skylights-factory.bin
```

To extract an OTA-only ESP-IDF app image (the format served for OTA updates, omitting the bootloader/partition table):

```sh
espflash save-image --chip esp32 --ignore-app-descriptor \
  target/xtensa-esp32-none-elf/release/skylights-app <version>.bin
```

### Flash Partition Layout (`partitions.csv`)

The 4 MiB flash is partitioned into symmetric 1856 KiB dual OTA slots (`skylights-core/src/ota/partition.rs`):

| Name | Type | SubType | Offset | Size | Purpose |
|---|---|---|---|---|---|
| `nvs` | `data` | `nvs` | `0x009000` | `0x005000` (20 KiB) | Non-volatile storage |
| `otadata` | `data` | `ota` | `0x00E000` | `0x002000` (8 KiB) | Two 4 KiB ping-pong OTA selection sectors |
| `phy_init` | `data` | `phy` | `0x010000` | `0x001000` (4 KiB) | RF PHY calibration data |
| `ota_0` | `app` | `ota_0` | `0x020000` | `0x1D0000` (1856 KiB) | Primary application slot |
| `ota_1` | `app` | `ota_1` | `0x1F0000` | `0x1D0000` (1856 KiB) | Secondary application slot |

### Streaming OTA & 30 s Self-Test Rollback

1. **Trigger**: `ota_task` runs once at boot and whenever an MQTT message arrives on `skylight/reset`.
2. **Fetch & Decision**: Connects to `OTA_URL` / `OTA_PROJECT` over **HTTPS** (TLS 1.3 via `embedded-tls`, optional HTTP Basic Auth) or **HTTP** (auth-free only; the firmware refuses to send Basic credentials over plain HTTP with `code=auth`). Fetches `version`; any integer differing from `SKYLIGHTS_BUILD_VERSION` (`remote != local`, including downgrades) triggers an update.
3. **Streaming & SHA-256 Gate**: Fetches `{version}.bin.sha256` (64 lowercase hex characters), then streams `{version}.bin` in 4 KiB sector-aligned blocks directly into the passive slot (`ota_0` <-> `ota_1`) while hashing. Any byte-count or read error leaves `otadata` untouched; on a SHA-256 mismatch, the passive slot's first sector is also erased and `otadata` is left untouched.
4. **Trial Boot & 30 s Self-Test**: On a valid hash, `otadata` is updated to trial-boot (`ESP_OTA_IMG_NEW`) and the chip resets. `self_test_task` arms the 8 s `TIMG1` watchdog and runs a **`30 s` verification window** (`SELF_TEST_WINDOW_MS = 30_000`) requiring all three signals:
   - `gpio_safe`: all 9 window outputs read `HIGH` at boot.
   - `wifi_connected`: station associated and DHCP lease acquired.
   - `mqtt_healthy`: `CONNECT` + `SUBACK` on `skylight/+` succeeded.
5. **Confirm or Rollback**: Passing within 30 s writes `ESP_OTA_IMG_VALID` to `otadata` and continues into `runtime_watchdog`. Failing `gpio_safe` or timing out at 30 s writes `ESP_OTA_IMG_INVALID` and resets so the ESP32 bootloader automatically rolls back to the previously valid slot.

### Release / OTA Publishing (`VERSION` & `deploy.sh`)

The published version is the single bare integer in the repository-root `VERSION` file (currently `8`). It must match `^[0-9]+$`. `deploy.sh` only **reads** `VERSION` — it never increments it. Bumping the version is an explicit repository commit made before deploying:

```sh
# edit VERSION, e.g. 8 -> 9
git add VERSION && git commit -m "release: VERSION 9"
./deploy.sh
```

#### `deploy.sh`

`deploy.sh` builds the release firmware, extracts the ESP32 app image, checks it against the 1856 KiB slot budget, hashes it, publishes the artifacts, prunes older remote pairs (keeping current + immediate predecessor), and triggers the device:

```sh
./deploy.sh --help      # usage, exits 0
./deploy.sh --dry-run   # full local build/extract/hash, prints the remote plan
./deploy.sh             # build, publish, and trigger
```

`--dry-run` performs the build, extraction, and SHA-256 hashing locally (so the printed digest is real) and prints the remote plan, but performs **no** `ssh`, `scp`, or `mosquitto_pub` — there are no network side effects. It is the recommended pre-flight check.

The image is produced with `espflash save-image --chip esp32 --ignore-app-descriptor`, which writes an ESP-IDF app image (with the required `0xE9` image header while intentionally omitting an ESP-IDF app-descriptor section). A raw `rust-objcopy -O binary` dump would omit that header and be rejected by the ESP32 OTA validator.

#### Configuration

`deploy.sh` is fully environment-driven. These are required:

| Variable | Meaning |
|---|---|
| `DEPLOY_PUBLISH_HOST` | `ssh`/`scp` target, e.g. `user@host` |
| `DEPLOY_PUBLISH_PATH` | remote base directory (nginx firmware root), e.g. `/var/www/firmware` |
| `DEPLOY_PROJECT` | remote project sub-directory; must equal the firmware's `OTA_PROJECT`, i.e. `skylights` |
| `DEPLOY_MQTT_BROKER` | MQTT broker host for the reset trigger, e.g. `broker` |

These are optional, with their defaults:

| Variable | Default |
|---|---|
| `DEPLOY_MQTT_PORT` | unset — `mosquitto_pub`'s own port |
| `DEPLOY_MQTT_USER` | unset (never echoed) |
| `DEPLOY_MQTT_PASSWORD` | unset (never echoed) |
| `DEPLOY_TRIGGER_TOPIC` | `skylight/reset` |
| `DEPLOY_TRIGGER_PAYLOAD` | `reset` |
| `DEPLOY_ENV_FILE` | unset — defaults to `$SCRIPT_DIR/.env.deploy` |

`DEPLOY_PROJECT` must equal the firmware's `OTA_PROJECT` (`skylights`). The remote layout under `$DEPLOY_PUBLISH_PATH/$DEPLOY_PROJECT` is:

| Path | Content |
|---|---|
| `version` | bare decimal integer, published **last** (the device's poll commit point) |
| `${version}.bin` | ESP32 app image (with ESP-IDF image header) |
| `${version}.bin.sha256` | exactly 64 lowercase hex characters of the `.bin` |

The `.bin` and `.bin.sha256` are uploaded first; `version` is written last via a temporary file plus `mv`, so the device never observes a partial release.

#### `.env.deploy`

Targets and credentials must never be committed — this is a public repository. Instead of exporting the variables, `deploy.sh` also loads an env file: `$SCRIPT_DIR/.env.deploy` if it exists, or the path given by `DEPLOY_ENV_FILE`. Example `.env.deploy` (placeholders only):

```sh
DEPLOY_PUBLISH_HOST=user@host
DEPLOY_PUBLISH_PATH=/var/www/firmware
DEPLOY_PROJECT=skylights
DEPLOY_MQTT_BROKER=broker
# optional
DEPLOY_MQTT_PORT=1883
DEPLOY_MQTT_USER=user
DEPLOY_MQTT_PASSWORD=secret
# DEPLOY_TRIGGER_TOPIC=skylight/reset
# DEPLOY_TRIGGER_PAYLOAD=reset
```

`.env.deploy` is gitignored; restrict it to your user:

```sh
chmod 600 .env.deploy
```

Precedence: exported environment variables override values from the file, so `DEPLOY_PROJECT=other ./deploy.sh` beats the file's `DEPLOY_PROJECT`.

---

## 6. Development, CI & Bench Verification

### Prerequisites

- **Pinned Rust toolchain** from `rust-toolchain.toml` (channel **`esp`**). Install the esp-rs fork with `espup`, which also installs the `xtensa-esp32-none-elf` target:

  ```sh
  cargo install espup --locked
  espup install
  . "$HOME/export-esp.sh"   # per shell, or add to your profile
  ```

- **`espflash`** for serial flashing and extracting OTA app images:

  ```sh
  cargo install espflash --locked
  ```

- **Release publishing tools**: `ssh`, `scp`, `mosquitto_pub` (`mosquitto-clients`), and `sha256sum` (`coreutils`).

### Site Secrets (`secrets.rs`)

Create the gitignored secrets file from the template before building:

```sh
cp skylights-app/src/secrets.example.rs skylights-app/src/secrets.rs
```

Edit `skylights-app/src/secrets.rs` to fill in `WIFI_SSID`, `WIFI_PASSWORD`, `MQTT_BROKER`, `OTA_URL`, `OTA_PROJECT`, `OTA_USER`, and `OTA_PASSWORD`. Note (`skylights-app/build.rs`):

- **Debug / dev builds** auto-copy `secrets.example.rs` to `secrets.rs` if missing (emitting a Cargo warning) so CI and clean checkouts compile.
- **Release builds** (`--release`) **panic** if `skylights-app/src/secrets.rs` is missing to prevent shipping firmware with placeholder credentials.

### Build, Lint & Test (CI Gates)

These commands mirror `.github/workflows/ci.yml` (host checks run on `stable`; firmware builds run on the pinned `esp` toolchain with `-Zbuild-std=core,alloc`):

```sh
cargo fmt --all --check
cargo clippy -p skylights-core --all-targets -- -D warnings
cargo test -p skylights-core
cargo build --package skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc
cargo clippy --package skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc -- -D warnings
cargo build --release --package skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc
```

### Bench Verification

Hardware-in-the-loop verification on a physical ESP32-WROOM uses the helper scripts under [`tests/bench/`](tests/bench/), covering serial `defmt` logging, MQTT parity checks, and local HTTP/HTTPS OTA artifact serving.

The bench host needs only `espflash`, `python3`, and `mosquitto-clients`; images are built on the development host and copied over. See [`tests/bench/README.md`](tests/bench/README.md) for wiring, `secrets.rs` bench settings, build/copy/flash commands, and helper script usage.
