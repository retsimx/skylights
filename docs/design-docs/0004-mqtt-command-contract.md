# 0004 — MQTT command/telemetry contract

- **Status**: Verified
- **Date**: 2026-09 (initial), 2026-11 (state version field)
- **Scope**: `skylights-core/src/mqtt.rs`, `skylights-app/src/mqtt.rs`, README §4

## Context

A home-automation controller already speaks MQTT. The firmware must accept
absolute position commands for three windows, report state, support an
emergency stop, and expose an OTA trigger. The broker is a LAN address; the
device has no UI and no persistent storage for position.

## Decision

Subscribe to `skylight/+` at QoS 0 and expose a compact, fixed-schema JSON
contract.

| Topic | Direction | Payload |
|---|---|---|
| `skylight/set` | inbound | `{"index":1..3,"percentage":0..100}` |
| `skylight/get` | inbound | `{"index":1..3}` |
| `skylight/get/response` | outbound | `{"index":N,"percentage":M}` |
| `skylight/stop` | inbound | `{"index":1..3}` |
| `skylight/reset` | inbound | any payload (raises `OTA_TRIGGER`) |
| `skylight/state` | outbound | fixed-order window array + `version`, `wifi_rssi`, `uptime_secs` |

Rules:

- `Index` is 1–3 (not zero-based); `Percentage` is 0–100, integer only.
  Out-of-range or malformed payloads are ignored and logged.
- Field order in `skylight/state` is normative (`StateMessage` in
  `skylights-core/src/mqtt.rs`).
- State is published immediately on every window state change and every 60 s
  heartbeat. There is no continuous position interpolation in telemetry; a
  moving window reports its last settled position with `moving:true`.
- `version` is the build-time `SKYLIGHTS_BUILD_VERSION`; it is how a release is
  verified.
- `skylight/reset` is permissive by design (any payload, including empty).

## Alternatives rejected

- **Zero-based indexes**: the user-facing convention is 1–3; converting at the
  edge would add a mapping and off-by-one risk.
- **Floating-point percentages**: the plant and model are integer; accepting
  floats would require parsing and rounding rules for no gain.
- **Retained state / QoS 1**: the device re-publishes on change and heartbeat;
  QoS 0 matches the LAN broker and avoids delivery-state complexity in a
  no_std client.
- **A separate telemetry schema per window**: one state message keeps the
  heartbeat atomic.

## Consequences

- Changing a topic, field name, field order or range is a breaking change:
  update `mqtt.rs`, its tests, README §4 and the bench `mqtt_check.sh` together.
- Position remains open-loop; `skylight/state` is an estimate, not a sensor
  reading. The `version` field is the only reliable release signal.
- The broker is trusted LAN infrastructure; there is no application-level auth
  in the firmware. See `docs/SECURITY.md`.
