# 0001 — Two-crate split with a host-testable no_std core

- **Status**: Verified
- **Date**: 2026-09 (initial firmware)
- **Scope**: workspace layout, dependency direction

## Context

The firmware must drive real hardware, but most of its risk is arithmetic and
policy: travel timing, position clamping, MQTT validation, OTA slot decisions,
reconnect backoff. Testing those on the ESP32 requires flashing, a UART and
physical access, which makes every iteration slow and every regression
expensive.

## Decision

Split the workspace into:

- `skylights-core` — `#![cfg_attr(not(test), no_std)]`, no HAL/ESP/allocator
  dependencies, all policy and pure logic. `serde` is optional.
- `skylights-app` — `#![no_std]`/`#![no_main]` ESP32 binary; pins, embassy
  tasks, Wi-Fi/net, flash driver, `secrets.rs`.

Dependency direction is one-way: `app → core`.

## Alternatives rejected

- **Single crate**: simpler layout, but every host test would pull the ESP
  target or require feature gymnastics, and the HAL would leak into policy.
- **Core with HAL traits**: would allow generic hardware tests, but adds
  type machinery for one board; the pure/impure boundary is already clean.
- **`std` core**: would make host testing easier but cannot run on the target;
  `cfg_attr(not(test), no_std)` already gives both.

## Consequences

- `cargo test -p skylights-core` exercises 127 tests in milliseconds on the
  host; CI can gate every change without hardware.
- Any core change must keep the no_std build green; CI builds the app for
  `xtensa-esp32-none-elf`.
- Hardware-only defects (GPIO, flash alignment, RF timing) still need the bench
  harness in `tests/bench/`.
