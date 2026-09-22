## 1. Release policy and requirements

- [ ] 1.1 Record complete issue/comment chronology and add the repository requirement-review rule.
- [ ] 1.2 Pin and validate git-cliff semantic configuration against isolated feature, fix, maintenance and breaking repositories.
- [ ] 1.3 Implement shared path classification and fail-closed application semantic validation, including mixed/deleted/reverted paths.

## 2. Version and publication implementation

- [ ] 2.1 Add vergen-gitcl metadata, non-authoritative Cargo placeholder and CLI version wiring with strict official-build checks.
- [ ] 2.2 Replace custom release machinery with serialized tag-first exact-source publication and incomplete-tag recovery.
- [ ] 2.3 Gate all main application compilation and retain successful required documentation PR checks.
- [ ] 2.4 Document release types, docs exclusions, retry procedure, version behavior and historical migration limits.

## 3. Acceptance verification

- [ ] 3.1 Test real git-cliff version decisions, wrong semantic types, complete unreleased ranges and tag/SHA invariants in isolated repositories.
- [ ] 3.2 Test stale/concurrent requests, per-commit queue draining, tag conflicts, post-tag failure, partial upload and completed-release retries without production tags.
- [ ] 3.3 Test actual workflow build gates and official metadata rejection, including missing/shallow/dirty source and cached metadata refresh.
- [ ] 3.4 Run host checks and both ARM builds; execute exact-tag fixture binaries under both target emulators to verify runtime/package agreement.
- [ ] 3.5 Re-read timestamped issue comments, reconcile every acceptance item and preserve relevant failures as fixtures.

## 4. Review and completion

- [ ] 4.1 Verify implementation against OpenSpec, sync canonical specs and archive this change in the implementation PR.
- [ ] 4.2 Publish grouped actual evidence comments with concise Summary-only PR body and obtain independent exact-head review/green checks before normal merge.
- [ ] 4.3 Observe the first real release after merge, verify tag/source/runtime provenance and close REM-30 with any material limits recorded.
