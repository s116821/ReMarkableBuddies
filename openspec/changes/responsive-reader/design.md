## Context and chronology
The original REM9 inventory suggested shortening 800/500/200ms waits and at least1s improvement. The September22 user clarification supersedes one-second-only acceptance: profile the whole interaction, reduce dominant status acquisition/restoration and other avoidable stalls, preserve UI-authoritative settings and safety, measure ordinary and primary/secondary Highlighter, errors and history. Full current description and all available comments were read. Comment b620fbfb (September22 16:55:56 UTC) adds repeated REM34 guarded availability failures; later planning comment9de722df records plan-only status. The REM9 description update at17:27:10.751UTC explicitly makes event/completion-driven sequencing the preferred architecture, superseding delay-reduction-first approaches. REM35 update at17:27:12.183UTC requires later Reader/Writer features to preserve it. Both updated descriptions and all available comments were reread (REM9 two comments; REM35 none). REM19 remains manual-dispatch-only and is excluded.

Two REM34 immediate diagnostic failed runs on runtime6e685 (same runtime as merged REM34) show capture-start to first stroke13.049/13.041s, first request13.542/13.537s,27 captures each. Request-to-response times7.991+3.198s and15.094+3.233s; failure44.057/51.599s. These are not trigger timings, successful-response/readiness timings, percentile estimates or universal benchmarks. Earlier REM32 nondefault primary/secondary acquisition+cleanup44.170/49.753s are separate runs; do not combine them with these figures. Refresh measurements on the actual baseline and retain complete runs.

## Goals / non-goals
Make local interaction visibly responsive without loosening page/ink/style/history ownership. Preserve exact model images and output text; attribute provider latency separately. Availability is part of responsiveness: a faster no-answer failure does not meet successful-Q&A acceptance. No blind sleep reductions, broader page masks, automatic stale-journal recovery, stale metadata authority, screenshot reuse across input mutations or unsupported Paper Pro claims.

## Measurement and budgets
Use monotonic phase events with a run ID, operation counts and elapsed times; logs must exclude credentials and image bodies. Bound observers and attempts. Measure trigger release (the existing2s qualification is separate), first visible status stroke, first provider request, each provider response, first/last visible answer and original-tools/page-ready completion. For immediate examples explicitly label capture-start instead of trigger-release. Separate screenshot discovery/read/conversion/resize/serialization, UI press/settle/observe, cleanup, navigation, keyboard and provider spans. Preserve prior runs and failures. A small diagnostic helper may consume native frames without model calls; it is not a new production flag or authority to mutate a tablet.

Initial target budgets to evaluate before closure: ordinary Fineliner first feedback <=3s and request start <=4s after trigger release; nondefault primary/secondary Highlighter <=5s/6s. Total status acquisition plus restoration/cleanup target <=6s ordinary and <=10s nondefault, with repeated worst cases reported; provider-response to first answer target <=5s on blank/existing eligible answer page, and tools-ready <=2s after final character. These are provisional engineering targets, not measured results or permission to leave avoidable10s stalls. At least1s happy-path saving remains required, but is insufficient alone. Baseline measurements must justify any revised budget in a reviewed plan amendment; unmet budgets and dominant avoidable stalls keep REM9 incomplete. Record fixed output text length and rendering cost so faster/shorter provider answers cannot masquerade as local improvement. Use at least3 repeated native runs per representative ordinary/nondefault slot condition, reporting individual values, median and max without claiming statistically meaningful p95 from3samples. Include typical/slow provider cases, failure and history checks. Paper Pro minimum delays remain explicitly unmeasured without hardware; build/equivalence coverage is distinct from native evidence.

## Decisions and implementation sequence
1. Inventory every wait and required completion predicate, verify available supported signals, and choose event/postcondition or bounded fresh-state fallback as detailed below. Establish a bounded phase inventory and baseline, including exact internal status baseline/rejected captures when opt-in dumping is enabled. REM34 refused twice at143,991. The retained external preattempt/settled pair also differs at586,824 and is NOT the lease pair. Overlay disappearance is unconfirmed; do not weaken the guard to accommodate a hypothesis.
2. Complementary cost-reduction candidate, not a replacement for completion-driven sequencing: refactor raw frame conversion into owned native image pixels. Current capture encodes nativePNG, re-encodes the same pixels, decodes, resizesNearest and encodes overview; StyleIo then decodes overview again. Normalize directly with the exact existing conversion/orientation/filter, serialize only where model/save APIs needPNG, and provide status image-only capture. Preserve public screenshot output, native strips and current discovery/session bracketing. This candidate must pass exact pixel comparisons before native use. Do not cache addresses or trust stale frames.
3. Profile remaining repeated observations/menu actions after the image path change. Reuse an observation only within a single unchanged state transition where equivalent identity/content checks can be proven; never across an input, erase, navigation or external edit. Keep checkpoint-before-input and every failure latch/rollback proof. Any changed convergence strategy uses bounded fresh observations and must distinguish transient UI from external content changes, not blanket ignore regions.
4. Address the repeated refusal from exact evidence. Add the actual pair and reachable fault/convergence case to regression coverage; preserve adversarial page/edit/slot/mutation failures. No repeated paid retries without new evidence. If evidence cannot safely distinguish redraw and user edits, retain conservative refusal and keep availability acceptance open instead of claiming fixed.
5. Replace navigation/settling/rendering sleep assumptions with supported completion events and verified postconditions, otherwise bounded fresh-state polling. Retain fixed waits only as documented protocol/physical/observability exceptions. Keep independent recognition and complete Q&A/history semantics. Avoid switching status tools during undo/redo.

## Pre-lease readiness decision from the exact native pair
The f38748d attempt in baseline.md isolates disappearing navigation chrome at
y991–1011. Before creating a lease/journal, establish positive inactive-footer
pixels in a fresh768x1024 observation: the bottom40 rows outside the left toolbar
must be white, except the observed single horizontal rule at y1009 which may be
uniformly black or white. This conservative predicate can decline status on pages
with real content in that margin; it never permits ignoring such content in an
active lease. The ordinary Reader fallback for unavailable status is retained.

Use a scoped existing Linux input observer to cancel on external input/device loss,
not to claim UI completion. Poll fresh status observations with a5s monotonic
deadline and50ms pacing, checking cancellation and deadline before and after each
observation. Require unchanged identity and all pixels above the footer during
the wait; a changed owner/page/session or other content is fatal, not a new baseline.
No input is emitted while waiting. Timeout suppresses unavailable status before
any journal/input, rather than treating elapsed time as readiness. Only the final
positively ready observation establishes the immutable lease baseline. Unknown
firmware/layout retains the existing unsupported-style fallback. Native availability,
observer cost and the precise predicate remain to be validated before acceptance.

Exercise immediate readiness, delayed disappearance, persistent overlay, unknown
footer ink, stale/wrong-owner observations, cancellation and slow observations
through this shared production wait. Repeated identical overlay frames are not
ready. Keep the exact old-pair strict active-lease refusal regression.

## Safety, regression and native gates
Toolbar transitions now issue exactly one journaled press and verify the expected
controls with fresh observations until a five-second monotonic deadline. Check
deadline and cancellation before and after observation; reject a late successful
frame. Pending states are paced at50ms, bounded by the remaining time. Duplicate
or stale nonmatching observations cannot cause another press. The phase/sequence
and immutable lease identity/baseline correlate this synchronous wait to its
operation; there is no unsupported asynchronous UI signal claim. Preserve durable
intent before input and existing recovery behavior on timeout or capture failure.

The native wait opens an all-input observer after touch release and checks it
around every capture. Cancellation/observer failure latches further status input
off, retains the recovery journal, and cannot be cleared by rollback. This observes
the no-input wait only: injected touches share the physical input source, so it
does not claim attribution of simultaneous external input during the250ms contact.
The former100ms post-release settling sleep is replaced by verified convergence;
contact duration remains a separately scoped physical-input candidate exception
requiring native validation. No active baseline refresh or repeated input retry.

The owned-image implementation converts each fresh native frame once. Encoded
workflow captures serialize that image once for native strips and once after
normalization; status observations and cleanup checks consume normalized owned
pixels directly, without PNG encoding/decoding. Both encoded buffers are cleared
before a capture attempt and published together only after both encodings succeed.
PID/allocation discovery still occurs for every capture. Status owner/session
bracketing and active-lease pixel guards are unchanged. The old codec/conversion
path remains test-only as an exact native/overview oracle across all three layouts.
The screenshot diagnostic's optional --image-only saves outside capture.total;
that additional artifact serialization is not part of the status capture timing.
New capture.raw_conversion and capture.native_png phases are disjoint; older
capture.native_png included conversion. Compare totals or appropriately described
phases, never sum nested historical spans as if they were exclusive.

Pure conversion tests compare the old codec pipeline and direct pixels across modern RM2BGRA (all neutral values, colored samples, random patterns, exact dimensions), legacy RM2 curves/rotation/flip and Paper ProRGBA including alpha. Keep malformed/truncated/ambiguous allocation failures. Runtime model/overview/detail images must decode to identical pixels; byte-level compression differences are not image differences. Test any failure leaves no stale usable capture. Existing native-frame fixtures remain strict; do not relabel synthesized input as human handwriting.

Simulator tests preserve actual workflow routing, exact output/delimiters, negative no-write/navigation policy, history ownership and modeled operation costs. Virtual elapsed time is not native latency. Add delayed/stale/UI-changing captures and failures only through reachable production paths; preserve all rejected fixtures. Meaningful full fmt/clippy/tests and both target builds precede merge.

Native testing requires separate advance notice and an idle authorized development RM2, exclusive writer, bounded processes, exact source/hash recording, original document/header/tool/service restoration, no firmware/pairing/sync. Preserve baseline/refusal evidence before changes. Repeated successful Reader runs, ordinary/nondefault primary/secondary tools, clear owned marks with neighbor preservation, precise restoration and undo/redo/failure paths are required; no success from a screenshot command alone. Full native task scope cannot be closed with only a microbenchmark. REM35 repeats integrated Reader+Writer performance/functional acceptance before1.0.

## Delivery and current constraints
Full plan precedes code; source/evidence review and green required CI, canonical sync and same-PR archive. Public contributors may run offline tests without private boards/keys/tablets; maintainer supplies authorized native/live gates. Application API authorization is separate from Codex usage limits. After personally resetting usage, the user reinstated a hard pause at10%remaining (90%used) in either applicable usage window. This supersedes the temporary consume-all authorization. Check current usage regularly, preserve recoverable checkpoints and restore safe native state before the threshold. Do not redeem resets, purchase credits, consume paid overage, bypass limits, or change unrelated automations. REM34 release v0.1.16 is fully verified. REM9 measurement and pre-lease readiness implementation are in progress; baseline.md records the bounded native attempt and restoration. Performance and successful-workflow gates remain open.

### Measurement review clarification
Input-write timestamps are command/event proxies, not proof of first visible pixels or last visible characters. Establish visible timings with bounded observation and report observer overhead separately; pending toolbar/menu manipulation is not useful status feedback. The provider-response-to-first-answer wall interval starts at the proposal response and includes subsequent independent transcription/other provider calls; report those provider spans separately from local residual time rather than silently excluding or attributing them to device work. Sum every status acquisition and cleanup across a complete iteration, not just one lease. The recorded REM34 first-stroke values are command-start proxies only.


## Completion-driven sequencing architecture
This is the default for REM9 and future Reader/Writer work. Inventory each wait with its caller, required state, supported observable signal, correlation key, verification predicate, deadline/cancellation path, and evidence for any fixed-wait exception. Investigate interfaces before claiming availability: xochitl is not assumed to expose reliable completion notifications. Prefer existing supported input/provider notifications plus verified state transitions; do not add an invasive tablet dependency or generic event framework merely to obtain events.

| Wait category | Completion predicate to establish | Signal investigation / fallback |
| --- | --- | --- |
| Virtual input initialization | Intended virtual device is ready for supported input delivery | Investigate existing readiness/registration observations; retain a bounded documented exception if no usable readiness signal exists. Creation/write success alone does not prove application consumption. |
| Page navigation | Requested destination belongs to the same document/session and fresh page content matches a completed transition | Investigate supported page/metadata notifications; verify current owner and fresh capture. Otherwise bounded fresh-state polling. A metadata event alone is insufficient. |
| Status selection/restoration | Actual slot, grid, color/width, menu and unchanged page satisfy the requested transition | Investigate supported UI notifications; current fresh UI classification is the fallback. Retain durable intent before each serialized mutation and verify exact postconditions. |
| Settled capture/header | A fresh correctly owned capture satisfies the required content/header predicate | Investigate supported invalidation/readiness signals; reobserve within a bound without reusing frames across mutation. Exact internal refusal frames remain required for diagnosis. |
| Text insertion/history | Expected native content and ownership are observed, with separate visible timing evidence | Investigate existing native document/metadata signals; correlate to the operation and verify exact content/postconditions. Keyboard writes or elapsed time are not completion. |
| Provider completion | The response for the active request is complete or failed/cancelled | Use existing request completion result; correlate request/iteration, preserve serialized device callbacks and distinguish provider time from local work. |

The future implementation must record supported mechanisms and evidence, not treat this investigation table as verified API availability. Any event must match the active operation/generation, page and session before its postcondition can advance state. Duplicate/out-of-order/stale observations cannot cause repeated input or completion of another operation. Lost signals trigger bounded fresh-state reobservation where safe; expiration is failure or an explicitly documented safe fallback, never success. Cancellation invalidates pending work and propagates failure through existing cleanup/recovery ownership rules, without concurrent input or stale callbacks reviving cancelled operations.

Fallback polling uses monotonic deadlines, cancellation checks and paced intervals; return immediately once a fresh correlated predicate holds. Bound observation cost and total work, avoid busy loops, and account for observer overhead. A new operation requires new observations; no cached frame across mutation boundaries. Event arrival is a reason to verify, not evidence of completion by itself.

Fixed delays require a recorded underlying protocol requirement, measured physical minimum, or unavailable observable predicate, with scope, bound and validation. Gesture qualification, animation cadence, polling pacing, retry backoff and timeout deadlines remain legitimate timing behavior. The target is replacing unsupported completion assumptions, not deleting all clocks. Existing safety checks, strict image comparison, history inactivity and recovery latches remain mandatory.

In the implementation PR add a concise AGENTS/workflow rule applying this default to future Reader/Writer features, with public/manual equivalents and no optional integration prerequisite. REM35 audits new waits and verifies preservation across the integrated system. Simulator coverage must exercise immediate/delayed completion, missing/duplicate/out-of-order/stale signals, wrong page/session/operation and cancellation via actual production sequencing; assert no duplicated mutation or stale successful transition. The architecture amendment preceded implementation; the measurement checkpoint now includes the AGENTS principle, while tasks.md tracks remaining implementation and validation.
