# Common Checklist

Applies to every change in this repository.

- [ ] **Tests**: Every behaviour change ships with a test at the lowest meaningful level; do not weaken or delete a failing assertion to pass. (❌ Anti-pattern: `assert!(true)` or deleting the failing case, ✅ Required: fix the behaviour or deliberately update the expectation with evidence) [Ref: skylights-core/src/window/tests.rs]
- [ ] **Format & lint**: `cargo fmt --all --check` and the CI clippy command pass before review. (❌ Anti-pattern: `#[allow(clippy::...)]` to silence a real lint, ✅ Required: fix the lint or justify a narrow allow) [Ref: .github/workflows/ci.yml]
- [ ] **Secrets**: Never stage `skylights-app/src/secrets.rs`, `.env.deploy`, keys or tokens. (❌ Anti-pattern: `git add -A`, ✅ Required: explicit paths only) [Ref: .gitignore]
- [ ] **Scope**: The diff contains only files required by the task; unrelated refactors and formatting are separate changes. (❌ Anti-pattern: drive-by cleanup, ✅ Required: one coherent change) [Ref: docs/CODE-REVIEW.md]
- [ ] **Docs**: User-visible behaviour changes update `README.md` in the same change. (❌ Anti-pattern: code says one thing, README another, ✅ Required: README and code agree) [Ref: README.md]
- [ ] **Dependencies**: A new crate must earn its complexity; prefer the existing mechanisms and check `Cargo.toml` first. (❌ Anti-pattern: adding a utility crate for one call, ✅ Required: reuse or justify) [Ref: Cargo.toml]
