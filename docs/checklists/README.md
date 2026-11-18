# Domain Checklists

Pre-flight and verification rules for this repository. Agents load the matching
domain file before implementing and cite `file:line` evidence against each item
when verifying.

## Files
- `common.md` — applies to every change.
- `backend.md` — Rust/no_std/ESP32/MQTT/OTA work in this repo.

## Item format

Every item MUST be one line in this shape:

```text
- [ ] **{Topic}**: {Rule} (❌ Anti-pattern: `{X}`, ✅ Required: `{Y}`) [Ref: {path_or_incident}]
```

`[Ref: ...]` must point at a real file or incident record. A dead reference is a
checklist defect.

## 3-state graduation lifecycle

1. **Incident** — a regression, bug or anti-pattern is found and recorded in
   `.agents/results/bugs/` (local).
2. **Active checklist** — a one-line guardrail is added here with the
   anti-pattern, required pattern and reference.
3. **Graduated** — once a test, clippy rule or CI check enforces it
   automatically, retire the manual item to preserve context headroom.

## Limits

- Hard cap: 20–25 active items per file. Prefer removing/graduating over adding.
- No generic advice ("write good code"); each item must be actionable and
  specific to this codebase.
