<!-- Parent: ../AGENTS.md -->

# skylights-app

> ESP32 `no_std`/`no_main` firmware binary: GPIO actuation, embassy tasks,
> networking, flash/OTA driver, and boot self-test.

## Constraints
- Target-only: builds for `xtensa-esp32-none-elf`; do not rely on host `std`.
- `src/secrets.rs` and `.env.deploy` are site-specific and gitignored. Never
  commit, print, or log credentials.
- All nine window outputs are active-low and must be HIGH before any slow init.
- Flash writes are 4 KiB sector-aligned; do not write arbitrary chunks.
- `ACTUATION_LOCK` serialises pulse bursts across windows; do not bypass it.

## Working Here
- Build with the esp toolchain:
  `. "$HOME/export-esp.sh"`,
  `cargo build --release -p skylights-app --target xtensa-esp32-none-elf -Zbuild-std=core,alloc`.
- A new task needs an embassy executor arena-size review and a spawn-failure
  log, matching the existing tasks.
- OTA changes must preserve the verify-then-commit order: hash first, then
  write `otadata`; never mark a trial image valid before its self-test.
- Release flow: bump `VERSION` in its own commit, `./deploy.sh --dry-run`, then
  `./deploy.sh`; verify `skylight/state`'s `version`.

## Dependencies
- Depends on: `skylights-core`.
- Depended on by: nothing (firmware image).

<!-- MANUAL: Notes below this line are preserved on regeneration -->
