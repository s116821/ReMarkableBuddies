## Context

The baseline is main b3dac64 (v0.1.4), after PR10. Four capability specs follow the
actual executable and tablet interfaces, including known defects and unused flags.

## Goals / Non-Goals

Goals: source-traceable contracts, official OpenSpec lifecycle, canonical specs
and an archived single baseline change.
Non-goals: runtime fixes, new features, new device testing, credential movement,
a separate documentation repository, or claims of untested hardware support.

## Decisions

- Group contracts by runtime, tablet I/O, analysis, and answer pages so subsequent
  deltas have stable ownership. A single giant spec was rejected as harder to review.
- Prefer executable source over comments/README: two-second hold, parsed-but-unused
  flags, current return retries, and loop error text are explicitly recorded.
- Preserve known defects as current behavior with ticket context. Fixing them here
  would invalidate baseline-only acceptance and blur later deltas.
- Use official OpenSpec 1.2.0 Codex templates. Init supplies core workflows; sync
  and verify skills are materialized unchanged from the installed official template
  exports, without changing the user's global OpenSpec profile.
- Store runtime contracts as canonical specs; keep provenance/review notes in this
  archive and PR comments. Do not import old run-log folders into specs.

## Risks / Trade-offs

- Describing a defect with SHALL can look like endorsement: mark known gaps and
  require later behavior changes to modify the relevant requirement explicitly.
- Source coverage is not hardware proof: retain precise PR10 evidence boundaries.
- Generated skill volume adds files: retain only core plus required sync/verify.
- Old roadmap summaries drift: current Linear issue bodies/relations govern scope.

## Migration Plan

Documentation only. Validate artifacts, review against source, sync four new
capabilities, archive the baseline, submit one PR, and merge after checks/review.
Rollback is a normal documentation revert; no device deployment is needed.

## Open Questions

No blocking baseline decision. Subsequent tickets own simulator fidelity, native
page insertion, new gestures, credentials and Paper Pro hardware acceptance.
