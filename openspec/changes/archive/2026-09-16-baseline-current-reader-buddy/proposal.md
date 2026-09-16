## Why

REM-27 establishes a current-state contract after the RM2 compatibility repair.
Future roadmap changes need a source-backed baseline rather than old feature plans.

## What Changes

- Configure official OpenSpec and install its Codex workflow skills locally to this repository.
- Describe implemented behavior at main commit b3dac64 (v0.1.4) in one baseline change.
- Document the proposal/design/specs/tasks, implementation, verification, sync and archive workflow.
- Sync this baseline into canonical specs and archive it within this documentation PR.
- Make no runtime or intentional product behavior change.

## Capabilities

### New Capabilities

These are newly documented contracts, not newly implemented features.

- `platform-runtime`: startup, configuration, service, logging and development boundaries.
- `tablet-io`: device detection, capture, input, gestures and coordinate assumptions.
- `reader-analysis`: current-page model proposal, strict parsing and independent reading.
- `reader-answer-pages`: navigation, page classification, text rendering and failure behavior.

### Modified Capabilities

None; no canonical specs existed before this baseline.

## Impact

OpenSpec artifacts, official skills and repository guidance only. No Rust source,
runtime dependencies, tablet state, credentials or service configuration changes.
Writer Buddy, retrieval, memory, sync, simulator and future gestures remain roadmap work.
