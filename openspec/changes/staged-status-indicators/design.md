## Context

Read REM-32 and its complete comment list, REM-8 description/all five comments, REM-4 description/all three comments and REM-15 description/all three comments. Relevant chronology (UTC): REM-8 Dec 8 2025 07:15 establishes initial/request-sent/response-received nesting; Dec 9 08:13 (updated 11:59) substitutes triangles, 333 ms per segment, retracing and tangent auxiliary circles. Dec 9 09:21 (updated 09:39) adds six failure segments; 11:00 requires cleanup before any viewport change and 11:04 excludes history activity. REM-4 Dec 9 11:58 retains center spokes for context-enhanced requests. Those spokes belong to REM-4. The REM-15 Sep 2026 native append-history availability report remains unresolved and is a validation constraint, not permission to relax ownership.

The Sep 22 audit initially missed REM-13 comment a4675fd3 (Dec 11 2025 00:58:41): model/network diagnostics must never be typed. Main 9c52933 orchestrator.rs:455-457 violates this in the outer loop. This correction includes repeated loop-mode provider, capture and marker-failure regressions; the earlier audit conclusion has been corrected.

The current backend draws one circle and erases that fixed path. Model waits sleep 750 ms after drawing, obscuring actual cadence. Loop errors can type arbitrary error text into the document. The replacement preserves region eligibility and thread serialization while making paths, phases and failure classifications explicit.

## Goals / Non-Goals

**Goals:** implement the complete requested geometry and stage progression, bounded ownership/cleanup, six observable failure categories and faithful local geometry/timing tests; verify native tools, neighboring ink and history interactions.

**Non-Goals:** retrieval/spokes implementation, follow-up recognition, page insertion, a new history protocol, firmware/account changes or a pixel overlay that pretends to restore document ink.

## Decisions

1. **Three nested triangles.** Preparing follows clean trigger capture; AnswerPending begins at answer-request dispatch; AnswerReady follows its response and remains the final stage until rendering/failure. Each triangle has three ordered edges, nested about a common center in the existing 50x50 region with native-stroke clearance. Queue stage advancement until the prior triangle has its first traversal. HTTP dispatch proceeds while that traversal finishes on the caller thread; a fast response can precede the displayed transition. Finish queued traversals before auxiliary inference/navigation so completed nesting is still visible. Subsequent pending ticks retrace that stage to build darkness; opacity depends on the selected native tool. Complete the ready traversal before clearing for navigation. This adds at most a few bounded 333 ms intervals after fast responses; drawing every edge at once would violate the requested cadence.

2. **Auxiliary inference.** Independent transcription uses a closed incircle tangent to the innermost displayed triangle. The circle is refreshed during that request, while completed triangle paths remain. Use shared pure geometry; REM-4 can later add per-edge center spokes without introducing another device writer. Do not add unused retrieval behavior now.

3. **Cadence and serialization.** Schedule stroke starts against a monotonic device clock at 333 ms intervals, accounting for time spent drawing. Backend delay advances virtual time in simulation and sleeps natively. The HTTP worker performs only transport; caller-thread progress uses deadline-based waits rather than sleeping 333 ms after a stroke. Slow strokes skip missed deadlines without a catch-up burst. Tests cover initial traversal, repeated edges, transitions, delayed HTTP and overrun calculation.

4. **Finite owned-path ledger.** Record each unique triangle edge/auxiliary circle before the draw attempt. Cleanup erases only these paths, including potentially partial strokes, with bounded work independent of wait duration. Successful cleanup clears the ledger. Failure latches the existing fatal cleanup condition and prevents navigation/output/future iterations. All current clean-capture, navigation, body-mode, text and completion boundaries retain cleanup. Suppressed corners neither draw nor acquire erasure ownership. Selected tool settings are never changed. Native pen/eraser release is attempted on failure.

5. **Six persistent failure codes.** Constant X plus exactly one segment of a centered 25x25 box: top = missing/unreadable/ambiguous selection or malformed answer proposal; right = independent transcription invalid/disagrees; bottom = provider transport/timeout/unavailable; left = no successor movement; horizontal midpoint = unsuitable successor after confirmed recovery; vertical midpoint = device/render/recovery failure. Cleanup precedes the mark; eligible state is required. Unknown/occupied corners suppress marks. Cleanup failure suppresses further input entirely. Provider errors still propagate in single-iteration mode, and verification-provider errors remain conservative declines, but each now attempts its proper code. Loop mode logs propagated errors and continues without inserting arbitrary Error text. Failed recovery uses device code rather than claiming a confirmed invalid-page return. Prevent duplicate markers within one iteration.

6. **Simulator model.** Replace the single circle raster with the same bounded stroke paths, accumulate retracing darkness deterministically, expose stage/path/timing events and persistent error segments. Update fault operation names and tests intentionally. Keep temporary marks separate from document text/lines. Assertions for captures/navigation/output and history remain strict. Native brush width, persistence and e-ink refresh cannot be proven by this model.

## Risks / Trade-offs

- Small nested geometry under a broad highlighter can merge visually -> inspect native Fineliner and Highlighter output and adjust within the fixed region; disclose actual tool limitations.
- More native strokes can affect persisted scene history -> save immediate pre-append/applied/undo states, test empty and occupied corners, preserve conservative scene guards and record availability failures rather than forcing success.
- Erasure can affect nearby ink -> retain the 12-pixel eligibility clearance and bounded owned paths, compare original native content and screenshots.
- Fast-stage minimum traversal adds latency -> bounded by missing first-pass segments; never sleep through provider work unnecessarily or run device operations concurrently.
- Device failure may leave no safe place for an error code -> log and propagate as applicable; safety takes priority over guaranteed visibility.

## Migration Plan

No stored content migration. Existing old marks remain user document content and make their corners ineligible. Update code/specs/simulator together, build both targets, run authorized disposable-document native tests and restore the original document/header/service. Roll back by restoring the previously verified binary. Publish full native/test evidence and obtain exact-head review plus required CI before normal merge.

## Open Questions

Native legibility and the existing append-history availability interaction require empirical validation before closure. The six error categories are the concrete implementation mapping because the original request specifies geometry/count but not named conditions.
