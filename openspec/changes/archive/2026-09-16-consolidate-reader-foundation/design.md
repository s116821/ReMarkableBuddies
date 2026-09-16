## Context

REM-13 follows the source-backed v0.1.4 baseline. It consolidates REM-6/2/14/3.
The upcoming navigation fix and simulator need testable, shared decisions.

## Goals / Non-Goals

Goals: lean justified configuration, reusable navigation/classification/composition,
deterministic behavioral regressions, useful standard logging, strict lint, and
a real-tablet sanity gate.
Non-goals: new features, changing retry count or delays, a simulator, new providers,
model prompt changes or new gestures.

## Decisions

- Keep screenshot-only/once/no-trigger as bounded hardware diagnostics. Remove
  inert image flags and misleading no-draw CLI rather than inventing fake offline
  behavior. Keep internal no_draw constructors for bounded diagnostic tooling.
- Use OPENAI_API_KEY exclusively for secrets; RUST_LOG for verbosity. Optional
  READER_BUDDY_DEBUG_DUMP retains existing image diagnostics without a CLI toggle.
  Reject invalid diagnostic booleans before touching devices.
- Extract pure classification and Q&A composition, plus one source comparison
  wrapper used by both forward and return verification. Preserve original timing,
  thresholds, masks and three-attempt return policy (REM-10 owns that change).
- Extract hold timing driven by monotonic durations, keeping Linux event routing.
  Test boundary/reset behavior without sleeps.
- Turn API decode unwraps into contextual errors and avoid logging full image
  request bodies; do not add retries or change model prompts.
- Existing CI already denies Clippy warnings; retain it and verify locally on
  native and ARM targets. A new lint framework is unnecessary.

## Risks / Trade-offs

- Removed CLI options break old diagnostic commands: update README and give
  explicit environment/diagnostic replacements.
- Refactor can change device behavior: require trigger/capture/live model,
  successful rendering/append and relevant rejection/occupied-page sanity on RM2.
- Pure tests cannot validate real input injection: record actual hardware evidence
  for the final candidate, with rollback and exclusive device ownership.

## Migration Plan

Set OPENAI_API_KEY in the existing environment file; use RUST_LOG for verbosity
and READER_BUDDY_DEBUG_DUMP only when collecting local page diagnostics.
Stage the tested ARM candidate with rollback, run bounded smoke, then restore
normal service state. Canonical sync/archive and all CI/review gates precede merge.

## Open Questions

No design blocker. Hardware connectivity and current document state must be
revalidated before mutation; a failed hardware gate prevents merging.
