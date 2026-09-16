## 1. Indicator and model wait

- [x] 1.1 Implement shared status geometry, occupied-region suppression and native/simulated circle operations with focused preservation tests.
- [x] 1.2 Add bounded progress-aware model waits, caller-thread callbacks and HTTP-only worker; test timing, errors and timeout cleanup.
- [x] 1.3 Integrate indicator ownership and cleanup across captures, navigation, typing, completion/rejection/error paths; resize guarded failure X.

## 2. Verification and delivery

- [ ] 2.1 Extend deterministic simulator assertions/fault cases and explicit live checks; pass full appropriate tests, formatting, strict lint and ARM builds.
- [ ] 2.2 Verify final build on disposable native document: visible recurring circle, blank answer/append, rejection/recovery/timeout cleanup, adjacent-ink preservation, and restore original document/cache/service.
- [ ] 2.3 Update docs with actual behavior and limitations; verify requirements/design coverage, sync canonical specs and archive in this implementation PR.

Final independent review, latest-head CI and automated-review results remain merge gates after archival.
