# SL-10 Bench Verification Checklist (issue #11)

Physical (hardware-in-the-loop) verification of the native Rust ESP32 firmware.
Sections A–I mirror the consolidated acceptance checklist on issue #11.

> **Part 1 (this run):** naked ESP32-WROOM, no GPIO wiring. Covers C–H plus
> pulse timing (A) and travel accuracy (B) measured **in-firmware**.
> **Part 2 (later):** physical relay/motor/window motion and any optional
> electrical waveform capture on production hardware.

## How to use

1. Complete the setup in `tests/bench/README.md`.
2. Capture UART from power-on: `tests/bench/serial_log.sh bench.log`.
3. Work top-to-bottom. For each case record `PASS`/`FAIL` and a short,
   **sanitised** evidence reference (the exact log line or capture snippet).
4. Keep raw logs out of the repository. Post sanitised evidence to issue #11.

**Sanitisation:** never record SSIDs, passwords, broker/OTA host addresses, or
tokens in this file or in any commit/comment. Replace them with `<broker>`,
`<ota-host>`, `<ssid>`, `<credential>`.

**Timing evidence format** (emitted by the firmware, `tests/bench/README.md`):

```
WINDOW <n>: actuation=<open|close|stop> pulses=2 low_ms=<a>,<b> high_ms=<c>,<d> settle_ms=<e>
WINDOW <n>: travel start target=<p> planned_ms=<ms> stop_after=<bool>
WINDOW <n>: travel end actual_ms=<ms> stop_pulse=<bool> position=<p>
WINDOW <n>: travel interrupted [elapsed_ms=<ms>|(retarget) elapsed_ms=<ms>] stop_pulse=true position=<p>
```

## Preconditions

- [ ] Device flashed with the Part 1 image (`Closes` firmware build `version` visible in `skylight/state`).
- [ ] Debug `secrets.rs` variant active (debug VLAN; OTA base URL points at the bench server).
- [ ] Broker reachable and `skylight/+` subscribed by the test client.
- [ ] Serial monitor running and tee'd to a log.

---

## A. Electrical & pulse timing

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| A1 | Boot idle invariant | Power on; observe from reset | All 9 pins (2,4,16,19,5,18,21,22,23) inactive (HIGH); log `GPIO boot probe: all window outputs inactive = true` | boot log | |
| A2 | Low pulse | `skylight/set {"index":1,"percentage":100}` | `actuation=open ... low_ms=75,75` (each 75 ±5) | serial line | |
| A3 | High pulse | as A2 | `high_ms=75,75` (each 75 ±5) | serial line | |
| A4 | Pulse count | as A2 | `pulses=2` | serial line | |
| A5 | Post-command settle | as A2 | `settle_ms=400` ±20 | serial line | |
| A6 | Interlock | Publish `set` to window 1 and window 2 back-to-back | The two `actuation=` lines serialise; no overlapping bursts (single `pulses=2` line at a time) | serial lines | |
| A7 | Pin mapping | For each window 1/2/3 issue open/close/stop | The `WINDOW n` line names the expected pin path (win1 2/4/16, win2 19/5/18, win3 21/22/23) | serial lines | |

## B. Positioning & travel parity

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| B1 | Full close | `skylight/set {"index":1,"percentage":0}` | `travel start ... target=0 planned_ms=71000 stop_after=false`; `travel end actual_ms≈71000 stop_pulse=false position=0` | serial + state | |
| B2 | Full open | `set {"index":1,"percentage":100}` | target=100, planned 71000, stop_after=false, end position=100 | serial + state | |
| B3 | Intermediate | from 0: `set {"index":1,"percentage":50}` | `target=50 planned_ms=35500 stop_after=true`; STOP pulse after travel; end position=50 | serial + state | |
| B4 | Mid-travel reversal | `set {"index":1,"percentage":100}`, then mid-course `set {"index":1,"percentage":20}` | First travel interrupted `(retarget)`, STOP pulse, interpolated position, then closes toward 20 | serial lines | |
| B5 | Emergency stop | during travel: `skylight/stop {"index":1}` | `travel interrupted elapsed_ms=<ms> stop_pulse=true position=<p>`; STOP pulse issued | serial + state | |
| B6 | Travel accuracy | all above | `actual_ms` within 1.0 s of `planned_ms` | serial lines | |
| B7 | Hard-seat recalibration | from 0: `set {"index":1,"percentage":0}` | Always emits full 71000 ms close with `stop_after=false` | serial line | |
| B8 | State reporting | after each move | `skylight/state` shows the settled `percentage` for that window | MQTT capture | |

## C. Boot, version & heap

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| C1 | Active slot | observe boot banner | `Active slot: ota_0`/`ota_1` reported | boot log | |
| C2 | Version + git hash | observe boot banner | `Booting Skylights firmware v<n> (<hash>)` | boot log | |
| C3 | Heap init | observe boot | Heap initialises, no panic; dynamic alloc verified | boot log | |
| C4 | Passive-slot erase/write | trigger OTA check against a valid image | Active slot stays bootable; passive slot written | serial log | |
| C5 | TLS allocation under load | complete an OTA download | 96 KiB heap suffices for TLS buffers | serial log | |

## D. Wi-Fi

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| D1 | Join on boot | power on in range | Associates and gets an IPv4 address unattended | serial log | |
| D2 | DNS | observe after join | `DNS resolved <ota-host> -> <ip>` | serial log | |
| D3 | Outage | block AP/broker ~60 s | No reboot/lockup; retries back off ≈1,2,4,8,16,30 s | serial log | |
| D4 | Recovery | restore AP/broker | Auto-rejoins with a working address, no reset | serial log | |
| D5 | Controls during outage | move a window during D3 | Movement completes normally | serial + state | |
| D6 | Bounded join | point at an absent SSID | `association attempt timed out`/`connect failed`; backoff 1,2,4,8,16,30 s; no wedge | serial log | |
| D7 | Dead-link rejoin | keep the AP up, block the broker | After 5 MQTT connect failures: `MQTT: repeated connect failures; requesting Wi-Fi rejoin` → `Wi-Fi: rejoin requested` → re-association | serial log | |
| D8 | No-IP reboot escalation | hold the device off-network past `NO_IP_REBOOT_MS` (shorten the constant locally for bench) | `Wi-Fi: no IP for <n>s; resetting to recover the radio`; reboot; then normal recovery | serial log | |
| D9 | In-place DHCP retry | slow/absent DHCP server | `DHCP not up yet; retrying on the same association` (up to 3 windows) before a full rejoin | serial log | |

> **Amended no-reset contract:** network loss no longer resets *except* the
> bounded no-IP escalation in D8, which only fires after `NO_IP_REBOOT_MS`
> (10 min) with no lease. Update the SL-6 wording accordingly.

## E. MQTT

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| E1 | Explicit subscribe | observe connect | `MQTT: connected, subscribed to skylight/+` | serial log | |
| E2 | Heartbeat | idle ≥60 s | `skylight/state` at least once per 60 s | MQTT capture | |
| E3 | Set | `skylight/set {"index":1,"percentage":50}` | Window 1 moves to 50% | serial + state | |
| E4 | Get response | `skylight/get {"index":2}` | Immediate `skylight/get/response {"index":2,"percentage":p}` (compact JSON) | MQTT capture | |
| E5 | Stop | `skylight/stop {"index":1}` | Moving window halts | serial + state | |
| E6 | Reset | `skylight/reset reset` | Device logs `MQTT: OTA reset requested` | serial log | |
| E7 | State on move | start then stop a move | `skylight/state` published at start and stop | MQTT capture | |
| E8 | Broker drop/restore | stop broker > retry window, restore | No reboot; reconnects and re-subscribes unattended | serial log | |
| E9 | State version field | inspect any `skylight/state` | `"version":<build>` equals the running firmware build version | MQTT capture | |

**Parity note:** legacy MicroPython emitted `{"index": 1, "percentage": 50}`
(spaces); the firmware emits `{"index":1,"percentage":50}`. Semantically
identical; whitespace is an accepted, documented divergence (no legacy
`skylight/state` telemetry exists to compare).

## F. OTA fetch & flash write

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| F1 | Happy path | serve v2 via `tests/bench/ota_http_server.py`; `skylight/reset` | `OTA: update available ...`; `OTA: ota_hash_ok`; `OTA: ota_marked resetting` | serial + server log | |
| F2 | TLS 1.3 (auth case) | serve over self-signed HTTPS with creds configured | Handshake succeeds; `Authorization` accepted; hash matches | serial + server log | |
| F3 | Chunking + hash | as F1 | Image streams in 4 KiB chunks; computed SHA-256 equals published `.sha256` | server log | |
| F4 | Oversize reject | serve a `.bin` > 1,900,544 B | Rejected before any erase/write (`oversize`); `otadata` unchanged | serial log | |
| F5 | Bit-flip abort | serve a `.bin` whose bytes differ from `.sha256` | `OTA: hash mismatch`; update aborts; active image still boots | serial log | |
| F6 | Server-side downgrade | local v2, serve v1 | Update triggered (remote != local) | serial log | |
| F7 | Same version | serve the running version | `OTA: version up to date ...`; no download | serial log | |
| F8 | Basic auth refused over HTTP | `OTA_BASIC_AUTH_USER` set, URL is `http://` | Update fails `code=auth` (never sends credentials in clear) | serial log | |
| F9 | Passive slot + otadata | complete F1 | Matching image written to passive slot; `otadata` updated before reset | serial + slot banner after reboot | |

## G. OTA confirm, rollback & self-test

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| G1 | Clean trial boot | boot a trial image | `gpio_safe` + Wi-Fi + MQTT within 30 s → `SELF-TEST: passed, slot confirmed`; `otadata` VALID | serial log | |
| G2 | gpio_safe probe | observe boot | `GPIO boot probe: all window outputs inactive = true` | serial log | |
| G3 | Panic rollback | flash a trial image that panics before confirm | Reboot → bootloader reverts to previous good slot | serial log | |
| G4 | MQTT-unreachable rollback | trial image with broker unreachable past 30 s | `SELF-TEST: failed reason=timeout`; reset for rollback | serial log | |
| G5 | Watchdog | hang the trial image | 8 s watchdog resets | serial log | |
| G6 | Confirmed-image skip | reboot a confirmed image | `SELF-TEST: skipped (slot already valid)`; no 30 s delay | serial log | |
| G7 | Power-cut during flash write | cut power mid-OTA write, restore | Previous version boots intact | serial log | |

## H. Release tooling on the real target

| ID | Test | Procedure | Expected | Evidence | Result |
|----|------|-----------|----------|----------|--------|
| H1 | Dry run | `./deploy.sh --dry-run` | Builds/extracts/hashes; prints remote plan; no network side effects | terminal capture | |
| H2 | Atomic publish | real publish to `<ota-host>` | `${VERSION}.bin`, `.sha256` uploaded; `version` written last | host-side capture | |
| H3 | Trigger delivery | `mosquitto_pub` trigger | Device receives and acts on `skylight/reset` | serial log | |
| H4 | Input validation | empty/non-integer `VERSION`; oversized image | Script aborts non-zero with a clear message | terminal capture | |

## I. Evidence & summary

| Section | Cases | Pass | Fail | Notes |
|---------|-------|------|------|-------|
| A | 7 | | | |
| B | 8 | | | |
| C | 5 | | | |
| D | 9 | | | |
| E | 9 | | | |
| F | 9 | | | |
| G | 7 | | | |
| H | 4 | | | |
| **Total** | **58** | | | |

**Deferred to Part 2:** physical relay/motor/window motion; optional external
electrical waveform capture; signed-manifest authenticity (#20).
