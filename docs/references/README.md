# External References

Pointers to the external crates this firmware depends on. Versions are pinned in
`Cargo.toml` / `Cargo.lock`; consult those before relying on an API shape.

| Area | Crates | Documentation |
|---|---|---|
| ESP32 HAL / radio | `esp-hal` 0.23.1, `esp-wifi` 0.12, `esp-hal-embassy` 0.6, `esp-storage` 0.4, `esp-alloc` 0.6, `esp-backtrace`, `esp-println` | https://docs.rs/esp-hal/0.23.1 , https://docs.rs/esp-wifi/0.12 |
| Async runtime | `embassy-executor` 0.7, `embassy-time` 0.4 (plus the 0.5 alias used by `embassy-net`), `embassy-sync` 0.6, `embassy-net` 0.8 | https://docs.rs/embassy-executor , https://docs.rs/embassy-net |
| MQTT | `minimq` 0.13 | https://docs.rs/minimq/0.13 |
| OTA transport | `embedded-tls` 0.19, `embedded-io-async` | https://docs.rs/embedded-tls/0.19 |
| Serialisation / hash | `serde-json-core` 0.5, `serde` 1, `sha2` 0.10 | https://docs.rs/serde-json-core , https://docs.rs/sha2 |

## Notes

- This is not an ESP-IDF project: `esp-hal` APIs and `espflash` image handling
  differ from ESP-IDF examples. Check the pinned version, not a blog post.
- `esp-storage` on ESP32 writes 32-byte blocks and does not split at 256-byte
  flash pages; the OTA driver buffers into 4 KiB aligned sectors for that
  reason.
- No LLM-reformatted vendor docs are vendored here. Add a
  `{library}-llms.txt` only if a dependency's docs are repeatedly needed and
  the extract can be kept current.
