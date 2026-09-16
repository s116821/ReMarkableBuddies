## Context

REM-13 supplies shared masked source-page verification. Invalid successors still
invoke a three-attempt reverse loop, so a delayed screen or failed swipe can cause
overshoot. REM-10 explicitly requires one attempt, immediate failure display, and
protection when the source is already visible.

## Goals / Non-Goals

Goals: bound recovery to one reverse swipe; verify source before and after it;
preserve the end-of-document forward guard; prove exact operation order with tests.
Non-goals: reliable native page identity, automatic page insertion, changing timing,
changing failure-X placement, simulator architecture or model behavior.

## Decisions

- Use a small internal navigation interface with `is_source` and `previous_page`
  operations. Production wraps Workflow plus its saved image; a scripted test
  double records calls. A numeric retry limit alone would leave regressions in
  the operation order untested; a general device abstraction belongs to REM-22.
- Return an explicit AlreadySource, Returned or Unconfirmed outcome. A precheck
  match does not swipe; a postcheck mismatch does not retry. Capture/input errors
  propagate to the existing render-error handler, which attempts the failure X.
- Expose the recovery operation on Workflow so orchestration and the existing
  bounded hardware probe exercise the same implementation. Keep thresholds/masks
  from REM-13 and the existing 800 ms post-swipe settling delay.
- Retain the forward no-movement check. The second check immediately before
  recovery protects against intervening screen transitions as requested.

## Risks / Trade-offs

- Screenshot identity remains heuristic -> preserve thresholds and document the
  limitation; unreliable identity is not solved by repeated swipes.
- One failed return may leave the X on the current successor -> this matches the
  requested immediate failure behavior; never navigate again merely to place it.
- Extra precheck adds one capture -> needed to avoid an unwanted reverse swipe;
  broader latency improvements remain REM-9.

## Migration Plan

No configuration migration. Test deterministically, build both ARM targets, then
stage the candidate with rollback on the authorized dev tablet. Use bounded
navigation checks for a normal successor and end of document. Preserve the PDF and
restore the normal service state. Sync/archive in the implementation PR; require
green latest-head CI and final review before normal merge.

## Open Questions

None. Record any hardware limitations without turning deterministic doubles into
claims of simulated physical input failure.
