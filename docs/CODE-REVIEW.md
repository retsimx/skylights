# Code Review Standards

## Checklist

1. **Scope** — does the diff contain only files the task requires? Are
   unrelated refactors separated?
2. **Tests** — does every behaviour change have a test at the lowest meaningful
   level? Were any assertions weakened or deleted?
3. **Invariants** — for this repo: core purity, absolute 0/100 boundaries,
   calibrated forward/inverse consistency, GPIO boot safety, verify-then-commit
   OTA.
4. **Arithmetic** — integer-only in core, bounded before casts, clamps at
   `0..=100` and `MIN_PULSE_MS`, no reachable overflow.
5. **Contract** — MQTT topics/schema/ranges and README §4 agree; release
   `VERSION` handling and deploy docs agree.
6. **Secrets** — no `secrets.rs`, `.env.deploy`, hostnames, SSIDs or tokens in
   the diff or logs.
7. **Docs** — README (and the matching design doc) updated when behaviour
   changes.

## Severity

| Level | Meaning | Gate |
|---|---|---|
| CRITICAL | Data loss, bricking, wrong actuation, secret exposure | Block merge |
| HIGH | Incorrect position/travel, broken rollback, failing invariant | Block merge |
| MEDIUM | Edge-case gap, missing test for a real branch, stale contract | Fix before merge or record |
| LOW | Naming, comment, wording, non-blocking cleanup | May follow up |

## Domain focus

- **`skylights-core`**: no HAL/ESP imports, no floats in the window model,
  boundary branch untouched, forward/inverse share `drive_curve`.
- **`skylights-app`**: pins HIGH before init, `ACTUATION_LOCK` around pulses,
  4 KiB-aligned flash writes, task spawn failures logged, no credential logs.
- **OTA/release**: hash before `otadata`, trial self-test intact, `VERSION`
  bumped in its own commit, verification uses `skylight/state`'s `version`.

## Anti-patterns to flag

- `git add -A` / `git add .` (adds `.agents/` and plan artifacts here).
- Silencing a failing test or adding a blanket `#[allow(...)]`.
- Editing generated/vendor files to make a check pass.
- A "small" fix that changes the MQTT schema without README/tests.
- Deploying without a version bump and calling it verified.

## Human review

Request human review for: OTA partition/`otadata` changes, GPIO pin remaps,
secret handling, release tooling, and anything that changes physical actuation
semantics. Everything else can merge on green CI plus agent review.
