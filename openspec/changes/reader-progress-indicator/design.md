## Context

Reader uses a synchronous model trait and a main-thread device backend. Simulator state is Rc<RefCell>; tablet pen/touch/keyboard actions must not overlap. Native pen marks are persisted annotations, so repainting framebuffer pixels cannot restore erased ink. REM-8 requests recurring feedback and cleanup, with a smaller failure X.

## Goals / Non-Goals

Goals: visibly signal active work, refresh during model waits, clean up on every normal/error path, maintain clean model/classification captures and preserve preexisting page content.
Non-goals: a framebuffer overlay compositor, new providers, human concurrent-input arbitration, crash-recovery transaction log, undo support or latency tuning outside the indicator mechanism.

## Decisions

- Use a shared 50x50 status region in virtual portrait coordinates, inset from the lower/right edges enough to avoid the page footer/UI. Draw the circle inside this region with padding; X uses the same bounds. Keep geometry and blank-region eligibility in one module and test it independently.
- Check a surrounding clearance area in the clean screenshot before using native pen marks. If occupied, unknown or unavailable, suppress only status drawing/erasing and continue Reader. This is preferable to indiscriminate region erasure or refusing the whole question. Once a circle is owned by the current workflow, clear it before any conflicting operation; never erase a region merely because it contains a mark. A previous X is preexisting content on the next iteration and is not blindly erased.
- The workflow owns indicator state. Draw after clean source capture, refresh during proposal and independent-transcription waits, and show activity on newly captured pages. Pause and clear before capture, navigation and keyboard operations, then resume only after a clean eligible page is known. Clear on completion/rejection/transport error/timeout and before failure X. Failure cleanup errors propagate; do not navigate after a failed clear. Suppression is logged and exposed by simulator events.
- Add a progress-aware LLMEngine method with a deterministic default, with OpenAI overriding it using an HTTP-only worker and timed main-thread callback. The worker never receives a device handle. Bound production HTTP requests to 90 seconds; explicitly configured simulator timeouts remain honored. Stop ticks on callback error, await the bounded worker and return the callback error without writing its answer. No automatic request retry or detached device worker. Model response/content/auth behavior remains unchanged.
- Simulator uses the same lifecycle and geometry, records indicator ticks/clears/suppression and can inject indicator failures. Temporary mark state is separate from persistent lines, enabling meaningful assertions that all captures/navigation/typing are circle-free and final successful output has no circle. Virtual ticks are deterministic; they are not network timing evidence.
- Preserve existing proposal-error propagation and loop Error text behavior after cleanup. Expected declines and render errors still attempt a failure mark, now safely guarded and 50x50. No unrelated error policy redesign.

## Risks / Trade-offs

- Native erasing may affect more than the nominal pen path -> require a surrounding blank clearance region, keep status geometry inset, and verify adjacent sentinel ink on the real tablet. If hardware cannot preserve nearby marks, stop and revise the mechanism before acceptance.
- Selected tool can affect native pen rendering -> verify circle visibility, cleanup and toolbar state on the final tablet build; do not infer from simulator pixels.
- Kill/power loss or concurrent manual annotation can leave temporary native marks -> document this limit; this issue does not claim transactional native undo or safe concurrent user editing.
- Drawing every tick can add latency -> one bounded circle per polling interval, no busy loop, pause during device operations and retain native evidence of useful responsiveness.
- Occupied corner suppresses visual status -> log fallback and keep the question workflow working. This protects existing content without inventing a blank-page prerequisite.

## Migration Plan

Maintain old runtime rollback, use a disposable technical paper and isolated bounded runs, test blank/append/rejection/occupied recovery plus visible slow-response ticks and failure cleanup. Restore document/header/service. Complete all checks, sync specifications and archive in the same PR; independent exact-head review and green CI precede merge.

## Open Questions

Final geometry clearance and active-tool behavior must be confirmed in the native acceptance run. These are implementation validation gates, not claims of tested behavior.

Cleanup failure poisons the current orchestrator: after same-page best-effort cleanup it returns an error and requires reinitialization, rather than erasing an owned mark after a user might have navigated. Any text-output attempt invalidates corner eligibility because partial typing may change the region; failure display is suppressed until a fresh clean capture establishes safety.

Native acceptance exposed overshoot from sparse fast pen points: the rendered circle exceeded its intended bounds and left a residual arc after clearing. Pace/interpolate the native continuous path at the existing line-drawing resolution, then repeat actual before/active/after and sentinel checks before acceptance. Keep this failed capture as evidence; simulator geometry does not prove native path fidelity.

The existing rectangular eraser also emits sparse, rapid endpoint strokes. For status cleanup, retrace the owned circle with the same paced continuous native path using the rubber tool, rather than sweeping arbitrary blank rectangle rows. This bounds erasure to owned geometry and avoids rapid tool toggles; native clearance and residual-mark checks still decide acceptance.

Native rejection exposed a one-pixel pen footprint beyond endpoint coordinates. Inset failure-X endpoints three pixels inside the shared status box, leaving stroke clearance while preserving the same guarded 50x50 region. Repeat native rejection on the final build to verify rendered bounds.
