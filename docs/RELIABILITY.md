# Reliability

## Boot invariants

1. All nine active-low actuation outputs are driven HIGH as the first operation
   after `esp_hal::init`, before heap, flash, Wi-Fi or logging.
2. `probe_gpio_safe` reads all nine back; a failure invalidates a trial image.
3. Position is RAM-only, so every reset homes all windows closed with a full
   71 s hard-seat drive. Homing is interruptible by MQTT commands.

## Watchdogs and trial boot

- A `TIMG1` watchdog is armed for 8 s during the post-swap self-test.
- The trial window is 30 s and requires `gpio_safe`, `wifi_connected` and
  `mqtt_healthy`. Pass → `ESP_OTA_IMG_VALID`; fail/timeout →
  `ESP_OTA_IMG_INVALID` and rollback.
- The same watchdog instance supervises runtime after confirmation.
- Reset reasons are logged at boot; a `CoreMwdt1` reset leaves no firmware log,
  so the next boot's banner is the only trace.

## Network supervision

- Wi-Fi join, DHCP, DNS, MQTT connect and TCP have explicit timeouts in
  `skylights-core/src/wifi.rs` (`JOIN_TIMEOUT_MS`, `DHCP_TIMEOUT_MS`,
  `DNS_TIMEOUT_MS`, `MQTT_CONNECT_TIMEOUT_MS`, `MQTT_TCP_TIMEOUT_MS`).
- Reconnects use `ReconnectBackoff`, reset only on a successful connection.
- No-IP and stale-progress watchdogs re-arm at link loss; a wedged client is
  recovered rather than assumed healthy.
- State is published on every window state change and every 60 s heartbeat, so
  a silent `skylight/state` stream for several minutes is a failure signal.

## OTA failure handling

| Failure | Behaviour |
|---|---|
| Version fetch/parse error | Log stable `code=`, leave `otadata`, retry on next trigger |
| Hash/byte-count mismatch | Erase passive slot's first sector, leave `otadata` |
| Flash/write error | Leave `otadata`; passive slot is not selected |
| Trial self-test failure | Mark invalid, reset, bootloader rolls back |
| Device offline at trigger | No update; next boot or `skylight/reset` retries |

## Actuator thermal limit

The OEM actuator has a thermal cutout (observed once after ~35 min cumulative
motor time at 76–89 % duty; recovered in ~2 min). Rated duty is roughly 20 %.
Consecutive 71 s boundary commands are seven times a typical partial move and
are the hazard.

Pacing is a host/bench obligation, not firmware:

- Space commands by `travel_ms + 5 s`, never a fixed interval.
- Avoid back-to-back boundary commands.
- Log cumulative motor seconds and abort on a full-length drive with no sash
  motion.

## Verification

- Host: `cargo test -p skylights-core`, clippy, fmt.
- Device: `tests/bench/README.md` — serial capture, MQTT checks, OTA fixtures.
- Release: `skylight/state`'s `version` after `./deploy.sh`; uptime past 30 s
  proves the trial confirmed rather than rolled back.
