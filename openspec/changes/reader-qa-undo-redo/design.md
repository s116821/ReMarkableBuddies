## Context

Reader types a separately composed Q&A after an existing or newly created header, then immediately returns to a blocking corner-trigger wait. Touch currently tracks slot zero only. Native pen progress and keyboard input share xochitl's edit history, so one global Undo cannot be assumed to equal one Q&A block. Native content files expose page IDs and last-opened state, but their update timing and active-document semantics are unverified.

## Goals / Non-Goals

Goals: remove/restore exactly the last successful Q&A without a model call; preserve prior content; recognize stationary four-/two-contact holds; forget ownership after departure or new iteration; expose failures and shared simulator traces.
Non-goals: persistent undo history, editing arbitrary older answers, native file replacement while xochitl owns documents, firmware/pairing/sync changes, page insertion, or silently undoing user edits.

## Decisions

- Add a pure history state machine with Empty, Applied and Undone states. A record contains exact rendered text, a page/session identity token and the expected current state. Only successful complete output creates Applied; partial output creates no usable history. Repeated matching gestures toggle state; redundant undo/redo is a no-op.
- Keep all native input serialized in the main workflow. Replace the idle trigger-only boundary with device interaction events (Reader trigger, undo hold, redo hold, invalidation). Start a new iteration by clearing the prior transaction, even if its later analysis fails. No history survives process restart.
- Use a multi-slot contact reducer evaluated at SYN_REPORT. Two/four contacts must remain stationary for two seconds with stable tracking identities; contact loss/count changes reset timing. Ignore transient two-contact states while forming four contacts. Emit once per hold and require all contacts released before rearming. Existing single-contact corner triggering remains supported.
- Candidate native block mutation: position at the known end of the owned text, select the exact rendered Q&A character range and remove it; redo retypes the retained block. Header text is not part of the range. This avoids relying on unproven native Undo grouping and preserves prior native annotations. Native diagnostic validation of cursor/end/selection/newline semantics is mandatory before adopting it; revise this design first if native evidence favors another exact-block mechanism.
- Guard every mutation with event-based ownership and a fresh native page/content check. Observe departure/edit input and native document notifications where they provide reliable identity. A swipe away and back must invalidate permanently, not be rescued by a similar screenshot. Persisted last-opened metadata or the loose Reader navigation similarity alone is insufficient. Unexpected input, unavailable identity, modified text, failed mutation or uncertain outcome invalidates the record and never triggers blind retries.
- During native feasibility, establish which page and edit events are timely and how two-finger native gestures interact with a long hold. If any signal is ambiguous, invalidate rather than attempt deletion. A supported implementation must still pass ordinary stationary two-/four-finger toggles; merely suppressing every gesture is not acceptance.
- Extend the simulator's explicit session action schema to drive the same history policy, including page departure/return, new iteration, edits, partial mutation and repeated toggles. Report stored state, active page, rendered text and ordered actions. Local simulation does not establish native text selection or xochitl gesture behavior.

## Risks / Trade-offs

- Native text selection can cross into older content -> bounded diagnostic with header, two distinct QAs and nearby ink; verify exact block removal/restoration and fail closed on any uncertainty.
- Native two-finger gesture can conflict -> test real input timing and release behavior before implementation acceptance; never send both native global Undo and a block deletion.
- Metadata can be delayed or coalesced -> event invalidation plus fresh identity; test fast departure/return and alternate navigation paths, not only a slow swipe.
- Intervening manual edits make a saved character range unsafe -> invalidate history on detected edits/uncertainty; do not overwrite them.
- Crashes or partial input can leave a partially changed block -> discard session and report failure; no blind compensating edit or disk snapshot replacement.

## Migration Plan

Use the REM-8 restored build as rollback. Draft all artifacts before diagnostic/production changes, then perform bounded tests on an isolated native clone with the normal service stopped. Validate native feasibility before wiring automatic mutations. Run shared tests, both ARM builds and live/native regression; restore original document/cache/service. Sync canonical specs and archive in the same PR; independent final-head review and green CI remain merge gates.

## Open Questions

Native gate must settle exact block key semantics, timely current-page identity/departure events, and native long-hold coexistence. These are investigation tasks with recorded pass/fail evidence, not assumptions that the candidate mechanism already works.
