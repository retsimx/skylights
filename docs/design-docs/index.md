# Design Docs

Durable architectural decisions for the Skylights firmware. Each record states
the context, the decision, the alternatives rejected, and the consequences.
Working plans and bench notes live in `docs/plans/` (local, gitignored).

| # | Decision | Status |
|---|---|---|
| [0001](0001-two-crate-no-std-core.md) | Two-crate split with a host-testable no_std core | Verified |
| [0002](0002-calibrated-affine-travel-model.md) | Calibrated affine travel model and interruption inverse | Verified in host tests; hardware acceptance pending |
| [0003](0003-dual-slot-ota-trial-rollback.md) | Dual-slot OTA with verify-then-commit and trial rollback | Verified |
| [0004](0004-mqtt-command-contract.md) | MQTT command/telemetry contract | Verified |

Core operating principles: [core-beliefs.md](core-beliefs.md).
