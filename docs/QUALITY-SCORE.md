# Quality Score

Baseline as of 2026-11-19. Grades reflect evidence available in the repository,
not aspirations. No composite scoring tool is configured; this is a manual
domain assessment.

| Domain | Grade | Evidence | Gaps |
|---|---|---|---|
| `skylights-core` | A | 127 host tests, 0 failed; clippy `-D warnings`; pure `no_std`; calibrated model covered by table, boundary, round-trip and clamp tests | No coverage tool; mid-open fit is extrapolated above ~25 % steps |
| `skylights-app` | B | CI builds and clippy-checks for `xtensa-esp32-none-elf`; OTA v9 deployed and confirmed on hardware; bench scripts exist | No host-testable app logic; full five-move positioning acceptance and thermal watch not recorded |
| Release / OTA | B+ | Verify-then-commit, SHA gate, 30 s trial, rollback design; deploy/verify flow exercised end-to-end for v9 | Rollback path not recently exercised on hardware; deploy depends on device being online at trigger |
| Documentation | B+ | README covers architecture, wiring, timing, MQTT, OTA, deploy; harness maps added | Historical design/plan artifacts are local-only; bench evidence is not checked in |
| Security | B | Secrets gitignored, release build fails without them, OTA hash + optional TLS, auth refused over plain HTTP | MQTT is anonymous LAN QoS 0; plain-HTTP OTA is integrity-only |

## Priority gaps

1. **Hardware positioning acceptance** (P0): five chained moves, ≤2 % per move,
   no accumulation; interruption check; thermal watch.
2. **Mid-open extrapolation** (P1): verify a ≥30 % mid-travel step; the 35.5 s
   sample is 0.42° below the line.
3. **Stroke-scale check** (P1): record seat-to-seat span; supplied validation
   angles imply ≈15.4–15.8° per 100 %, the model uses 14.0°.
4. **Coverage tooling** (P3): add `cargo llvm-cov` (host only) if coverage
   becomes a gate.

## How to update

Change a grade only with evidence (test output, CI run, bench log). Record the
date and the gap that closed or opened. Do not raise a grade because a change
"looks fine".
