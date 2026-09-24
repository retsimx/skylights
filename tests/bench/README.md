# Skylights bench harness

Tools and procedure for the hardware-in-the-loop verification in
[`../bench_checklist.md`](../bench_checklist.md). Replace every `<placeholder>`
with your site value; **never commit or echo secrets, SSIDs, or host addresses.**

## 1. Hardware

- **Target:** ESP32-WROOM-32 dev board with a USB-UART bridge (CH340/CP2102).
  One USB-C cable to the bench host. For Part 1 leave all GPIOs disconnected.
- **Serial device:** usually `/dev/ttyUSB0` (CH340). Confirm with
  `ls -l /dev/ttyUSB*`; a `/dev/ttyACM*` is *not* the CH340.
- **Network:** the device joins the bench/debug VLAN via its `secrets.rs`.
- **JTAG is not required.** Everything here is observed over UART, MQTT, and
  the OTA server. A spare Raspberry Pi Pico flashed with `debugprobe` can serve
  as a CMSIS-DAP JTAG probe later (classic ESP32 is JTAG, not SWD) if a hang
  ever needs live debugging.

## 2. Bench-host software

- `espflash` — flash + monitor. Install the release binary, e.g. for aarch64:
  `curl -sSL -o /tmp/e.zip https://github.com/esp-rs/espflash/releases/download/v4.6.0/espflash-aarch64-unknown-linux-gnu.zip && unzip -o /tmp/e.zip -d /usr/local/bin`
- `python3` — OTA artifact server (standard library only).
- `mosquitto-clients` — `mosquitto_pub` / `mosquitto_sub` (`apt-get install -y mosquitto-clients`).

The esp toolchain and repository are **not** needed on the bench host: the
image is built on the development host and copied over.

## 3. `secrets.rs` selection

`skylights-app/src/secrets.rs` is gitignored and site-specific. Keep one variant
per network. For a bench run use the debug variant (same VLAN as the bench host)
and set:

- `OTA_BASE_URL` = `http://<bench-host>:8000` (plain HTTP tests), **and**
  `OTA_BASIC_AUTH_USER = None` — the firmware refuses to send Basic credentials
  over non-TLS, so an HTTP URL with a user configured fails with `code=auth`
  (that refusal is itself checklist F8).
- For the HTTPS/auth case, build a second image with
  `OTA_BASE_URL = https://<bench-host>:8443` and credentials set.
- `MQTT_HOST` = the broker address reachable from the debug VLAN.

## 4. Build and copy (development host)

```sh
. "$HOME/export-esp.sh"
cargo build --release -p skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc

# Full factory image: bootloader + partition table + app (for the first flash).
espflash save-image --chip esp32 --merge --partition-table partitions.csv \
  target/xtensa-esp32-none-elf/release/skylights-app /tmp/skylights-v1-factory.bin

# OTA app images for the server (header-only app image, as deploy.sh produces).
espflash save-image --chip esp32 --ignore-app-descriptor \
  target/xtensa-esp32-none-elf/release/skylights-app /tmp/1.bin

mkdir -p /tmp/skylights-bench && cp /tmp/skylights-v1-factory.bin /tmp/1.bin /tmp/skylights-bench/
scp -r /tmp/skylights-bench tests/bench root@<bench-host>:/root/
```

To produce a v2 image, bump `VERSION` to `2` in a **local, uncommitted** edit,
rebuild, and extract `2.bin`; then restore `VERSION`.

## 5. Flash and monitor (bench host)

```sh
export BENCH_SERIAL=/dev/ttyUSB0
espflash write-bin --chip esp32 --port "$BENCH_SERIAL" 0x0 /root/skylights-bench/skylights-v1-factory.bin
/root/tests/bench/serial_log.sh /root/bench-$(date -u +%Y%m%d-%H%M%S).log
```

`serial_log.sh` decodes the firmware's defmt frames and tees to a log. The
firmware logs the pulse and travel timings used for checklist sections A and B.

## 6. MQTT checks

```sh
export MQTT_HOST=<broker> MQTT_PORT=1883
/root/tests/bench/mqtt_check.sh
```

Covers checklist E2–E7 and E9 automatically. E1/E6/E8 are confirmed from the
serial log. Egress is limited to the `skylight/*` topics.

## 7. OTA checks

```sh
mkdir -p /root/ota-root
# normal v1 -> v2 while running v1:
/root/tests/bench/ota_check.sh stage /root/ota-root 2 /root/skylights-bench/2.bin
/root/tests/bench/ota_http_server.py --dir /root/ota-root --port 8000 &

# trigger the device (MQTT reset):
mosquitto_pub -h "$MQTT_HOST" -t skylight/reset -m reset
```

Scenario fixtures (see `ota_check.sh --help`):

| Scenario | Stage command |
|----------|---------------|
| oversize | `stage /root/ota-root 2 <2.bin> --oversize 2000000` |
| bit-flip | `stage /root/ota-root 2 <2.bin> --sha-of <1.bin>` |
| downgrade | `stage /root/ota-root 1 <1.bin>` (while running v2) |
| same-version | `stage /root/ota-root <current> <current>.bin` |

For the Basic-auth case, add `--auth <user>:<pass>` and a self-signed cert:

```sh
openssl req -x509 -newkey rsa:2048 -nodes -days 7 \
  -keyout /root/ota-key.pem -out /root/ota-cert.pem -subj "/CN=<bench-host>"
/root/tests/bench/ota_http_server.py --dir /root/ota-root --port 8443 \
  --auth <user>:<pass> --tls-cert /root/ota-cert.pem --tls-key /root/ota-key.pem &
```

## 8. Evidence and sanitisation

Record raw logs only on the bench host. In the checklist and in any issue/PR
comment, cite only the relevant line and redact SSIDs, addresses, and
credentials.
