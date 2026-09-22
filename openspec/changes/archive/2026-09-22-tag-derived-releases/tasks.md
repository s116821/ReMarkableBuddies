## 1. Release policy and requirements

- [x] 1.1 Record complete issue/comment chronology and add the repository requirement-review rule.
- [x] 1.2 Pin and validate git-cliff semantic configuration against isolated feature, fix, maintenance and breaking repositories.
- [x] 1.3 Implement shared path classification and fail-closed application semantic validation, including mixed/deleted/reverted paths.

## 2. Version and publication implementation

- [x] 2.1 Add vergen-gitcl metadata, non-authoritative Cargo placeholder and CLI version wiring with strict official-build checks.
- [x] 2.2 Replace custom release machinery with serialized tag-first exact-source publication and incomplete-tag recovery.
- [x] 2.3 Gate all main application compilation and retain successful required documentation PR checks.
- [x] 2.4 Document release types, docs exclusions, retry procedure, version behavior and historical migration limits.

## 3. Acceptance verification

- [x] 3.1 Test real git-cliff version decisions, wrong semantic types, complete unreleased ranges and tag/SHA invariants in isolated repositories.
- [x] 3.2 Test stale/concurrent requests, per-commit queue draining, tag conflicts, post-tag failure, partial upload and completed-release retries without production tags.
- [x] 3.3 Test actual workflow build gates and official metadata rejection, including missing/shallow/dirty source and cached metadata refresh.
- [x] 3.4 Run host checks and both ARM builds; execute exact-tag fixture binaries under both target emulators to verify runtime/package agreement.
- [x] 3.5 Re-read timestamped issue comments, reconcile every acceptance item and preserve relevant failures as fixtures.

## 4. Review and completion

- [x] 4.1 Verify implementation against OpenSpec, sync canonical specs and archive this change in the implementation PR.

External completion gates remain after this implementation archive: publish grouped
actual evidence comments with a concise Summary-only PR body, obtain independent
exact-head review and green checks, then merge normally. Before closing REM-30,
observe the first real release and verify tag/source/runtime provenance. The archive
records completed implementation and fixture verification, not a claim that these
post-commit and post-merge gates have already completed.
