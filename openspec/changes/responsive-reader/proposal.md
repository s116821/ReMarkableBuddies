## Why
Reader interaction contains long local stalls and repeated guarded refusals. REM9's September22 clarification requires full interaction responsiveness, not just a one-second reduction in old sleeps; REM34 also carries two unresolved no-answer availability failures.

## What Changes
- Prefer supported completion events plus verified postconditions; otherwise use bounded fresh-state polling with deadlines and cancellation instead of sleep-based completion assumptions.
- Measure capture, tool verification, provider, navigation, rendering and ready-state costs separately with bounded reproducible diagnostics.
- Eliminate redundant image serialization and repeated work while preserving exact pixels and ownership/session checks.
- Investigate repeated native status refusal using exact internal frames; retain strict pre-mutation and cleanup safety.
- Define and test explicit latency budgets across ordinary/nondefault primary/secondary tools, success, failures and history.
- Establish concise repo-local AGENTS guidance for future Reader/Writer sequencing and document necessary fixed-wait exceptions.
- Extend simulator timing/state coverage and publish before/after evidence, retaining unverified hardware limits.

## Capabilities
### New Capabilities
- `reader-responsiveness`: measurement boundaries, budgets and reliable progress/readiness.
### Modified Capabilities
- `tablet-io`: equivalent direct pixel normalization and image-only observation path.
- `local-simulator`: modeled operation timing and state convergence regressions.
- `development-testing`: completion/state-driven sequencing guidance for future features.

## Impact
Screenshot conversion and status observation, bounded diagnostic examples, workflow timing/state transitions where evidence justifies changes, regression fixtures and public guidance. No provider/prompt redesign, key change, Writer feature, account operation or release1.0. REM35 retains integrated Reader+Writer acceptance.
