## Why

REM-15 lets readers remove a just-rendered answer that is unhelpful and restore it without another model call. The current Reader loop forgets the answer after typing and has no page-scoped undo ownership.

## What Changes

- Retain one in-memory transaction for the last successfully rendered Q&A block on its native answer page, preserving the header, older answers and unrelated ink.
- Recognize a stationary four-finger two-second hold for undo and a two-finger two-second hold for redo; require release before another action and keep the existing Reader trigger working.
- Permit repeated undo/redo while the session remains valid; invalidate on page departure, a new Reader iteration, uncertain ownership or intervening edits that make removal unsafe.
- Use observed page/input events and a pre-action ownership check so stale state cannot edit a different page. Do not assume persisted metadata alone provides immediate active-page identity.
- Extend deterministic simulator sessions, native diagnostics and failure coverage. Validate native text boundaries and gesture coexistence before shipping.

## Capabilities

### New Capabilities
- reader-qa-history: page-scoped last-answer undo/redo transaction and invalidation policy.

### Modified Capabilities
- reader-answer-pages: register successful Q&A output as one reversible block, excluding existing header and earlier content.
- tablet-io: multi-contact hold events, release/rearm and page-departure observation alongside the existing corner trigger.
- local-simulator: replay undo/redo sessions, departures, intervening edits and failed mutations through shared production history policy.

## Impact

Touch event reduction, keyboard/native text editing, device backend event surface, workflow/orchestrator loop and simulator schema/reporting. An explicit native feasibility gate determines reliable block editing and departure signals; update design before any mechanism changes. No persistent history, global document undo, automatic page insertion, firmware change, pairing or sync.
