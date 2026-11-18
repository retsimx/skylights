<!-- Parent: ../AGENTS.md -->

# skylights-core

> Pure, hardware-agnostic policy library; the host-testable half of the firmware.

## Constraints
- `#![cfg_attr(not(test), no_std)]`: no `std`, no HAL, no ESP crates, no
  allocator, no clocks or tasks.
- Integer math only in the window model; no floats.
- Public API is re-exported from `src/lib.rs`; keep modules private unless
  callers need them.
- `serde` support is feature-gated (`default = ["serde"]`) and must stay
  `no_std`-compatible.

## Working Here
- Add behaviour with a host test in the same module (`#[cfg(test)] mod tests`).
- Keep the window model's forward and inverse mappings sharing one curve
  selector; a change to one without the other is a bug.
- Boundary targets (0/100) belong to `WindowPositioner::request`, not to
  `travel_duration_ms`.
- Run `cargo test -p skylights-core` and
  `cargo clippy -p skylights-core --all-targets -- -D warnings`.

## Dependencies
- Depends on: nothing internal; `sha2`, optional `serde`/`serde-json-core`.
- Depended on by: `skylights-app` and its build script.

<!-- MANUAL: Notes below this line are preserved on regeneration -->
