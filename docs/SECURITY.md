# Security

## Assets

- Wi-Fi, MQTT and OTA credentials in `skylights-app/src/secrets.rs`.
- Deployment targets and any broker credentials in `.env.deploy`.
- Firmware integrity and the physical windows (an unwanted drive is a safety
  event, not just a bug).

## Trust boundaries

```text
[repo] --build--> [image] --scp--> [OTA file root] --HTTP(S)--> [device]
[operator] --MQTT--> [LAN broker] --MQTT--> [device]
[device] --active-low pulses--> [OEM remote] --> [actuator]
```

- The MQTT broker is trusted LAN infrastructure. The firmware has no
  application-level auth or TLS for MQTT.
- The OTA file root is trusted; the device fetches `version`, `sha256` and the
  image from it.
- Physical access to the remote/actuator bypasses the firmware entirely.

## Controls

- **Secrets**: `secrets.rs` and `.env.deploy` are gitignored; release builds
  fail if `secrets.rs` is absent; deploy logs redact `-u`/`-P` values.
- **OTA transport**: HTTPS is supported (TLS 1.3 via `embedded-tls`) with
  optional Basic auth. Plain HTTP is allowed only without credentials; the
  firmware refuses to send Basic auth over plain HTTP (`code=auth`).
- **Integrity**: the complete image is SHA-256 verified before `otadata` is
  written; a mismatch leaves the running slot selected.
- **Recovery**: a trial image must pass GPIO/Wi-Fi/MQTT health within 30 s or
  the bootloader rolls back.
- **Safe boot**: all actuation pins are HIGH before slow init, with a probe.
- **Logging**: log event names and stable `code=` values; never SSIDs,
  passwords, tokens or full broker URLs with credentials.

## Residual risks

- MQTT commands are unauthenticated on the LAN; anyone with broker access can
  drive windows, stop motion or trigger OTA checks. Restrict broker access and
  network segmentation at the deployment site.
- Plain-HTTP OTA provides integrity only if the file root is trusted; it has no
  confidentiality. Prefer HTTPS with auth where the server supports it.
- There is no firmware image signature beyond the SHA-256 published by the same
  OTA root; compromising that root allows arbitrary firmware.
- A physical OEM remote can always operate the window.

## Rules

- Never commit, print, or paste credentials, hostnames or `.env.deploy`
  contents into issues/PRs.
- If a secret leaks, rotate it and treat the old value as compromised; git
  history is public.
- New network surfaces need a documented threat model before merge.
