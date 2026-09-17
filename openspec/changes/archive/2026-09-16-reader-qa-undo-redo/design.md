## Context and scope

Reader owns only the last completely rendered Q&A in its current process and native page visit. Native typing, pen status marks and user edits share xochitl history, so an arbitrary global Undo cannot safely identify that Q&A. The feature must preserve the header, earlier answers, formatting and unrelated ink, and make no model request when toggling.

The initial native contract is RM2 firmware 3.28.0.172. Other devices and firmware retain ordinary Reader operation without history. Cross-compilation does not establish hardware support. History is memory-only; there is no file replacement, persistent transaction recovery, automatic compensation, page insertion, pairing or firmware change.

## Ownership and state

A shared state machine tracks Empty, Applied, Undone and an in-flight mutation. A record stores the exact appended ASCII block, parsed before/applied content and native document, page, visit and process-session identity. Only a complete append with preserved prior content can arm a record. Ownership is bounded before arming to 2000 ASCII characters and 32 newline-delimited paragraphs. Larger answers still render normally.

The native adapter continuously observes all input devices, excluding only its exact owned virtual keyboard identity. It observes complete Protocol-B SYN_REPORT frames, starts from a complete kernel slot snapshot, and invalidates on external edits, navigation, unexpected controls, device inventory changes, incomplete input, SYN_DROPPED or lost devices. All mutation input remains serialized on the workflow thread. Per-key checks require quiet input, unchanged current document/page/visit and the same xochitl PID/start time.

Current document identity comes from xochitl's LastOpen setting, combined with cPages page identity and visit revision. Newest lastOpened metadata is diagnostic only: a native counterexample selected the wrong document. LastOpen can be empty after a native restart even with a document visible; the adapter refuses history without guessing. Reopening may restore identity, but only a new successful Reader iteration can arm a new record. Returning to a page never revives an old record.

The bounded RMv6 parser rejects malformed CRDT order, duplicate IDs, dangling anchors and unsupported structure. It compares all prior visible text/styles, root layout/opaque bytes and every non-text scene record. Native PageInfo counters are normalized only after validation. An empty insertion paragraph may change body style; visible prior paragraphs may not.

Native persistence is delayed by about 10–11 seconds in observed runs. The adapter waits at most 30 seconds for the complete expected parsed content, then requires three identical snapshots while still observing input. A stable but stale file does not arm history. Unavailable observation does not turn a correctly rendered Q&A into a failed Reader answer.

## Native mutation

The first undo retains Reader's uninterrupted insertion cursor after the complete append. It holds Ctrl+Shift and sends Up once per owned paragraph, with 50 ms key-event pacing, releases modifiers and deletes that range. It never guesses an end position with CtrlEnd or a fixed number of CtrlDown events. The 32-paragraph cap bounds selection to 64 arrow events, about 3.2 seconds plus guards; a 60-second input deadline is defensive, not normal size handling.

This exact first deletion establishes a known native history entry. Redo uses native CtrlZ to restore that deletion; subsequent undo uses CtrlY to reapply it. Retyping was rejected because it changed formatting. Every transition must match the complete expected native text and preservation contract before success is reported. Failure discards history without retry or compensating edits. Such discard limits further damage but does not make a partial deletion acceptable normal behavior.

RM2 caret composition uses Shift+6 followed by Space. Native testing exposed reordered expressions when adjacent input outran composition; typing now allows 50 ms before composition and after its Space commit. Exact rendered content still gates history, so unexpected operator output never arms a record.

## Gestures and idle integration

Two stationary contacts request redo and four request undo after two seconds, emitted only after every contact is released. Tracking identities, bounded formation/release windows and jitter checks prevent transient two-contact states during four-contact formation from requesting redo. Full committed frames drive timing; cancellation or uncertain input invalidates ownership.

The existing lower-left Reader hold retains its two-second threshold and now dispatches on release. A native test exposed menu reopening when dispatch/dismiss occurred while the contact was still down. Initial observer failure disables history and retains the legacy Reader trigger rather than causing service restart churn.

The orchestrator waits at a shared interaction boundary. It handles undo/redo without beginning a model iteration, preserves history while idle, and discards it before any new Reader iteration, status edit, navigation or unrelated output. Invalidations take priority over history actions. A Reader event remains usable when delivered with an invalidation.

## Evidence and rejected approaches

- Naive per-character selection crossed an earlier separator in one cursor setup. CtrlEnd did not establish the end. Retyping changed paragraph styling.
- Expanded Edit-text diagnostics passed short and longer character selections, but the same fast selection in the normal inline Reader editor deleted only a suffix. The real inline partial result is a regression fixture that must fail completion; it is not acceptance evidence.
- Visually inspected paragraph selection removed exactly the first inline Q&A, leaving its header; native restore preserved full parsed text/styles and non-text records. A 32-paragraph normal-inline append completed two exact undo/redo cycles.
- A full Reader run recognized the real handwritten uncertainty question twice, rendered source-specific values and units, then native four-/two-contact holds removed/restored the complete Q&A. External keyboard movement invalidated history and a later undo left the native page file byte-identical.
- The earlier reordered-operator native fixture must fail arming. Missing identity, wrong-page metadata, delayed persistence and partial mutation remain explicit simulator or parser regressions.
- The 2000-character wrapped block passed two exact normal-inline undo/redo cycles. Host tests, host/ARM strict lint, both ARM release builds and original document/cache/pen/service restoration passed. Synthetic native events do not establish physical-finger behavior or Paper Pro support.

## Simulator and delivery

The simulator drives the same history and contact policies through explicit session actions and reports ordered mutations, text, page and state. Faults cover stale/delayed/wrong/unavailable observations, partial mutations, errors, manual edits, departure/return, restart and input loss. It models paragraph boundaries but cannot prove native Ctrl+Shift+Up selection, persistence timing, physical gestures or handwriting recognition. New native findings update shared fixtures, faults, assertions and relevant OpenSpec deltas in the same PR, per AGENTS.md.

Validate on isolated disposable technical-paper copies with the normal service stopped. Preserve original PDF bytes and restore the agreed original document, header cache, pen and service. Keep the installed REM-8 build as rollback until final acceptance. Run host tests, strict host/native ARM lint, both ARM builds and final native/model regression; verify/sync/archive OpenSpec in the implementation PR. Fresh independent final-head review and green CI/automated review remain merge gates.
