## 1. Single-attempt recovery

- [x] 1.1 Implement source precheck and at-most-one reverse swipe using the shared identity helper.
- [x] 1.2 Add operation-order regressions for already-source, success, failed return and input/capture errors.
- [x] 1.3 Extend the bounded hardware probe and document recovery behavior.

## 2. Verification and delivery

- [x] 2.1 Pass formatting, tests, strict lint and ARM builds.
- [x] 2.2 Run focused dev-tablet navigation checks and restore service state with attributed evidence.
- [x] 2.3 Verify code/specs, sync canonical deltas and archive in the implementation PR.

Normal merge additionally requires latest-head CI and final review; track that
post-archive delivery gate in the PR and MVP progress record.
