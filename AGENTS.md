# Skylights — ESP32 Window Controller Firmware

> Bare-metal Rust (`no_std`) controller for three Velux skylights: OEM-remote
> pulse actuation, MQTT command/telemetry, and dual-slot OTA with trial-boot
> rollback.

## Architecture
See [ARCHITECTURE.md](ARCHITECTURE.md) for the crate/task map and data flow.

## Documentation
- [Design Docs](docs/design-docs/index.md) — durable decisions and core beliefs
- [Plans](docs/PLANS.md) — design/work conventions (`docs/plans/`, local only)
- [Product Specs](docs/product-specs/index.md) — no separate specs; README is the behaviour contract
- [References](docs/references/README.md) — external dependency pointers

## Domain Guides
- [Reliability](docs/RELIABILITY.md) — watchdogs, OTA rollback, reconnect
- [Security](docs/SECURITY.md) — secrets, OTA transport, MQTT trust boundary
- [Checklists](docs/checklists/README.md) — pre-flight and verification rules
- [Code Review](docs/CODE-REVIEW.md) — review standards and severities
- [Quality Score](docs/QUALITY-SCORE.md) — current domain grades and gaps

## Project Structure
- `skylights-core/` — pure, host-testable policy: window model, MQTT codec, OTA
  decisions, Wi-Fi backoff, version parsing. `#![cfg_attr(not(test), no_std)]`.
- `skylights-app/` — ESP32 binary: GPIO, embassy tasks, networking, flash/OTA
  driver, `secrets.rs` (gitignored).
- `tests/bench/` — hardware-in-the-loop scripts: MQTT checks, OTA fixtures,
  serial capture.
- `docs/` — harness docs. `docs/plans/` is local working notes (gitignored).

## Commands
```sh
cargo test -p skylights-core
cargo clippy -p skylights-core --all-targets -- -D warnings
cargo fmt --all --check

# Firmware (requires the esp toolchain)
. "$HOME/export-esp.sh"
cargo build --release -p skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc
./deploy.sh --dry-run   # build + extract + hash, no network
./deploy.sh             # publish + MQTT OTA trigger
```
CI: `.github/workflows/ci.yml` runs host fmt/clippy/tests and the xtensa build
plus clippy.

## Quick Rules
- Keep `skylights-core` free of HAL, ESP and allocator dependencies; it must
  build and test on the host.
- Never commit `skylights-app/src/secrets.rs` or `.env.deploy`.
- Boundary targets 0/100 always drive the full 71 000 ms with
  `stop_after = false`; partial moves use the calibrated affine mapping.
- Stage explicit paths only. `git add -A` here would add `.agents/` and plan
  artifacts.
- OTA only triggers when `VERSION` differs from the running build: bump it in
  its own commit before `./deploy.sh`.
- Verify a release from `skylight/state`'s `version` field; `skylight/get` has
  no version field.
- Do not weaken a failing test to make a change pass; fix the behaviour or the
  expectation deliberately.

<!-- MANUAL: Notes below this line are preserved on regeneration -->
