# 0002 — Calibrated affine travel model and interruption inverse

- **Status**: Verified in host tests; five-move hardware acceptance pending
- **Date**: 2026-11-18 (replaced the flat 710 ms/% model)
- **Scope**: `skylights-core/src/window.rs`, `README.md` §3

## Context

Position is open-loop: the controller commands a drive time and trusts the
calibration. The original model used one constant, 710 ms per percentage point,
for every partial move, and interpolated interruptions as
`elapsed_ms * 100 / 71_000`.

Bench characterisation of window 3 (GY-521 on the sash, 100 Hz, tilt against a
closed-seat baseline, sensor zero drift removed) showed the plant is affine but
**not one line**. Twelve full strokes fitted at R² 0.9996–0.9999 over a 14.0°
usable stroke. Banded rate variation was ±5 % (0.3 % of stroke), below the fit
residual, so an affine model is preferable to a lookup table.

## Decision

Three measured inversions, selected by start position and direction:

| Case | Drive time for a ΔV step | Fit | Residual |
|---|---|---|---|
| Opening from the closed seat | `645.2·ΔV + 207` ms | `Δθ = 0.2170·D − 0.045` | sd 0.0335°, max 0.0506°, n=6 |
| Opening mid-travel | `677.3·ΔV − 1374` ms | `Δθ = 0.2067·D + 0.284` | sd 0.0307°, max 0.0814°, n=11 |
| Closing mid-travel | `660.5·ΔV − 1262` ms | `Δθ = −0.2120·D − 0.267` | n=2 |

- Coefficients are signed integer tenths (`6452`, `2070`, `6773`, `-13740`,
  `6605`, `-12620`) with one final `/10`; no floats.
- `MIN_PULSE_MS = 200` floors partial pulses. The negative mid-travel
  intercepts mean a 1–2 % step needs zero/negative time; it still pulses, never
  becomes a no-op.
- `interrupted_position` inverts the **same** curve for the motion's start
  position and direction, rounds to nearest, and clamps `0..=100`.
- Boundary targets 0/100 stay outside this mapping: exactly 71 000 ms,
  `stop_after = false`, hard-seat re-zero.

## Alternatives rejected

- **Keep 710 ms/%**: over-drives seat steps by ~10 % (14.2 s vs 13.1 s at 20 %)
  and mid-travel steps by 14–31 %, because it ignores the per-move over-travel.
- **Lookup table**: banded rate variation is smaller than the fit residual;
  a table adds interpolation error and memory without improving accuracy.
- **Deadband/backlash terms**: the smallest tested command (1 %, 710 ms)
  produced 1.14 % of stroke, and a 3.55 s close after an open moved −1.02°
  against +1.03° opening. Neither effect is measurable.
- **Plant temperature compensation**: fits were consistent across 43.6–50.5 °C
  die; the +0.0619 °C⁻¹ drift was the sensor zero, removed before fitting.

## Limits and open evidence

- The seat fit spans 0.71–7.10 s; the mid-travel fits are anchored at
  3.55–7.10 s. Steps above ~25 % are an extrapolation. The single 35.5 s
  cross-check sits 0.42° (≈3 % of stroke) below the line.
- An interrupted full-stroke boundary drive uses the partial curve and can read
  a few points away from the linear 71 s fraction; the next boundary command
  re-zeroes.
- The five-move acceptance run (1–99 %, ≤2 % per move, no monotonic
  accumulation) is still outstanding. It must also record the seat-to-seat
  angle span: the supplied validation's absolute angles imply ≈15.4–15.8° per
  100 % span, while the model uses 14.0°.
- Hardware pacing is a bench obligation: space commands by `travel_ms + 5 s`,
  never a fixed interval, and avoid consecutive 71 s boundary commands (the
  actuator has a thermal cutout; rated duty ≈20 %).

## Consequences

- A general Stop can no longer use one divisor; it must select the curve. The
  zero-elapsed mid-travel case reads ±2 % (the model's per-move over-travel);
  the seat curve reads 0.
- Any change to a coefficient must update both directions and the round-trip
  test; the shared `drive_curve` selector is the guard.
- README §3 is the user-facing statement of this model; keep it in sync.
