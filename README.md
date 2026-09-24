# skylights — ESP32 firmware (native Rust / embassy)

## Overview

The Skylights firmware controls a Velux window: it reports state over MQTT,
accepts open/stop/close commands, and updates itself over the air. This
repository is the **ESP32 firmware**, written in native Rust (`no_std`) on
`esp-hal` and `embassy` for the ESP32 (`xtensa-esp32-none-elf`). It is a Cargo
workspace of two crates:

- `skylights-core` — host-testable policy: the OTA session and partition state
  machine, the MQTT topic/payload contract, window value rules, and version
  parsing. It depends on no hardware-facing crates, so its tests run on the host.
- `skylights-app` — the firmware image: WiFi/network bring-up, the MQTT client,
  window control, and the streaming HTTPS/TLS OTA fetch that writes the inactive
  OTA slot and confirms the boot after a swap.

The device OTA-fetches three files from a web root: a bare-integer `version`, an
ESP32 app image `${version}.bin`, and a `${version}.bin.sha256` sidecar. It is
woken to apply an update by an MQTT reset trigger published to
`skylight/reset`. The published artifact layout and the read-only `VERSION`
contract are described in [Release / OTA publishing](#release--ota-publishing).

## Prerequisites

- The pinned Rust toolchain from `rust-toolchain.toml` (channel **`esp`**).
  Install the esp-rs fork with `espup`, which also installs the
  `xtensa-esp32-none-elf` target:

  ```sh
  cargo install espup --locked
  espup install
  . "$HOME/export-esp.sh"   # per shell, or add to your profile
  ```

- `espflash` for serial flashing and for extracting the release app image:

  ```sh
  cargo install espflash --locked
  ```

- For release publishing: `ssh`, `scp`, `mosquitto_pub` (mosquitto-clients), and
  `sha256sum` (coreutils).

A clean checkout must materialise the (gitignored) secrets file before it
builds — CI does the same:

```sh
cp skylights-app/src/secrets.example.rs skylights-app/src/secrets.rs
```

## Build / test / lint

These mirror `.github/workflows/ci.yml`. The host commands run on the stable
toolchain; the firmware build runs on the pinned `esp` toolchain and needs
`-Zbuild-std=core,alloc` to build the `core` and `alloc` standard-library
crates for the bare-metal target.

```sh
cargo fmt --all --check
cargo clippy -p skylights-core --all-targets -- -D warnings
cargo test -p skylights-core
cargo build --package skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc
cargo clippy --package skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc -- -D warnings
```

The first four are the `host` job's gates plus the firmware build; the last is
the `firmware` job's app-clippy gate. Run them all before opening a pull
request. `cargo run` flashes and monitors the board over USB via the `espflash`
runner configured in `.cargo/config.toml`.

## Release / OTA publishing

The published version is the single bare integer in the repository-root
`VERSION` file (currently `1`). It must match `^[0-9]+$`. `deploy.sh` only
**reads** `VERSION` — it never increments it. Bumping the version is an explicit
repository commit made before deploying:

```sh
# edit VERSION, e.g. 1 -> 2
git add VERSION && git commit -m "release: VERSION 2"
./deploy.sh
```

### `deploy.sh`

`deploy.sh` builds the firmware, extracts the ESP32 app image, hashes it, then
publishes the artifacts and triggers the device:

```sh
./deploy.sh --help      # usage, exits 0
./deploy.sh --dry-run   # full local build/extract/hash, prints the remote plan
./deploy.sh             # build, publish, and trigger
```

`--dry-run` performs the build, extraction, and SHA-256 hashing locally (so the
printed digest is real) and prints the remote plan, but performs **no** `ssh`,
`scp`, or `mosquitto_pub` — there are no network side effects. It is the
recommended pre-flight check.

The image is produced with `espflash save-image --chip esp32`, which writes an
ESP-IDF app image (with the required image header). A raw `rust-objcopy -O
binary` dump would omit that header and be rejected by the ESP32 OTA validator.

### Configuration

`deploy.sh` is fully environment-driven. These are required:

| Variable | Meaning |
|---|---|
| `DEPLOY_PUBLISH_HOST` | `ssh`/`scp` target, e.g. `user@host` |
| `DEPLOY_PUBLISH_PATH` | remote base directory (nginx firmware root), e.g. `/path/to/firmware` |
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

`DEPLOY_PROJECT` must equal the firmware's `OTA_PROJECT` (`skylights`). The
remote layout under `$DEPLOY_PUBLISH_PATH/$DEPLOY_PROJECT` is:

| Path | Content |
|---|---|
| `version` | bare decimal integer, published **last** (the device's poll commit point) |
| `${version}.bin` | ESP32 app image (with ESP-IDF image header) |
| `${version}.bin.sha256` | exactly 64 lowercase hex characters of the `.bin` |

The `.bin` and `.bin.sha256` are uploaded first; `version` is written last via a
temporary file plus `mv`, so the device never observes a partial release.

### `.env.deploy`

Targets must never be committed — this is a public repository. Instead of
exporting the variables, `deploy.sh` also loads an env file: `$SCRIPT_DIR/.env.deploy`
if it exists, or the path given by `DEPLOY_ENV_FILE`. Example `.env.deploy`
(placeholders only):

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

Precedence: exported environment variables override values from the file, so
`DEPLOY_PROJECT=other ./deploy.sh` beats the file's `DEPLOY_PROJECT`.
