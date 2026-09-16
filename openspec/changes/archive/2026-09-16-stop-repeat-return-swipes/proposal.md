## Why

REM-10 addresses repeated reverse swipes after an unverified return, which can move
several pages away from the question. At the document end, recovery must also avoid
swiping backward when the forward attempt never left the source.

## What Changes

- Check source-page identity before attempting a reverse swipe.
- Make at most one reverse swipe and one post-swipe verification; on failure draw
  the existing X without another navigation attempt.
- Retain the forward no-movement guard, existing masks, thresholds and settling delays.
- Cover the recovery sequence with deterministic navigation doubles and focused
  real-tablet checks.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `reader-answer-pages`: bounded single-attempt return and explicit already-source guard.

## Impact

Workflow return orchestration, a small testable recovery boundary, behavioral tests
and documentation. No new CLI switches, simulator, model prompt changes, page creation
or changes to other roadmap features.
