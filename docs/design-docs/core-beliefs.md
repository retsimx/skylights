# Core Beliefs

How work is done in this repository.

1. **Policy is pure; hardware is an adapter.** All arithmetic, parsing and
   state machines live in `skylights-core` and run on the host. `skylights-app`
   owns pins, tasks, network I/O and flash.
2. **The host test suite is the contract.** A behaviour change without a test
   is incomplete. Hardware verification is the final check, not the first
   feedback loop.
3. **Boundaries are absolute.** The closed and open hard seats re-zero the
   open-loop position. Every reset homes closed; a boundary command always
   drives the full stroke.
4. **Verify, then commit.** OTA hashes the whole image before touching
   `otadata`, boots it as a trial, and only confirms after a 30 s health
   window. Failed trials roll back automatically.
5. **Secrets never enter the repository.** Wi-Fi/MQTT/OTA credentials live in
   gitignored `secrets.rs`; deployment targets live in `.env.deploy`.
6. **Small, coherent, explicit changes.** Stage explicit paths, keep refactors
   separate, and prefer the existing mechanism over a new abstraction.
7. **Measure before modelling.** The travel model is affine because bench data
   said so (R² ≥ 0.9996), not because a line is convenient. Keep the measured
   residual and range next to the constants.
