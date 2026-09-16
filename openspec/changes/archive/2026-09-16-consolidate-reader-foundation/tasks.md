## 1. Foundation cleanup

- [x] 1.1 Implement lean startup/configuration and document environment migration.
- [x] 1.2 Extract source-page verification, page classification and Q&A composition.
- [x] 1.3 Isolate hold timing and remove API decode panics/image request dumps.

## 2. Verification and delivery

- [x] 2.1 Add/run meaningful config, page, formatting, hold and API response regressions.
- [x] 2.2 Pass formatting, strict Clippy and ARM builds without changing retry policy.
- [x] 2.3 Coordinate exclusive tablet access and pass final-candidate real-device smoke.
- [x] 2.4 Verify implementation against deltas, sync canonical specs and archive.
- [x] 2.5 Prepare implementation and acceptance evidence for required CI and final review.

Delivery gate after archival: pass latest-head CI and fresh final review before normal
merge, then update Linear/progress. This post-archive gate is tracked in the PR and
MVP progress record; archival does not claim the PR has already passed CI or merged.
