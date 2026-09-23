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
### Navigation completion after the captured scrollbar refusal
On the verified RM2 firmware contract, resolve the adjacent native page before
one swipe from the validated current ordered cPages list. Preserve source
document/page/visit/session and the complete active ordering/redirect map through
the operation. Inserted notes pages are real neighbors; PDF redirect numbers are
not navigation indices. Reject duplicate/ambiguous IDs, unsupported entries and
mutated ordering. A known boundary issues no swipe. Input failure must release
the owned touch, and timeout must never repeat the swipe.

Use a fresh source image bracketed by its owner/order. After the swipe, poll
fresh target images with the same brackets, an all-input observer around reads,
a five-second monotonic deadline and50ms pacing. Target UUID must be the resolved
neighbor in the same document/session with a new visit. Old source metadata or
old source pixels cannot complete navigation. A wrong page/session/order cancels;
after target observation, a different visit cannot silently establish a new target.
The input observer covers read-only periods before/after the gesture; direct
physical-device injection still prevents attribution during the owned gesture.

Require positive clear footer and white x735..739 gutter over rows61..983 on the
verified layout, plus two matching fresh target candidate images within tolerance8
and a visual difference from the source using the existing page comparison.
Legitimate ink in that gutter conservatively prevents readiness; no pixels are
ignored or erased. Explicitly test/measure this availability limit across the
technical paper, Q&A and blank successor. Persistent/stale states time out with a
specific reason rather than silently adding a recurring sleep.

This composite of identity, fresh pixels and positive chrome is observable
readiness evidence, not a native render acknowledgement or independent pixel-to-page
provenance. Delayed metadata and delayed/partial rendering are separate faults;
keep downstream eligibility and immutable status/content guards. Identical-looking
adjacent pages conservatively refuse, including blank-to-blank; an inked Reader
question source to a blank successor is distinguishable and must pass. A stable
source after the deadline is no movement, not successful destination readiness;
the existing verified-source failure handling remains available without a retry.

Scope this new completion contract to verified RM2 firmware3.28.0.172. Other
hardware/firmware retains its existing heuristic/timing compatibility behavior
with explicit unvalidated limits; no cross-device completion claim. Initially
retain the caller's800ms wait while native evidence is collected. Before removing
that stacked wait, expose an explicit completion result/capability so unsupported
fallback paths preserve their sequencing. The production wait must have shared
deterministic tests for delayed metadata/render, stale/duplicate reads, wrong
neighbor/session/visit, order/redirect mutation, input cancellation and slow reads
past the deadline. No change to either readiness's strict content comparison or
an active lease baseline is authorized by the scrollbar evidence.

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

History persistence diagnosis must retain the actual pre-render native content and
owner, requested append bytes, and observed post-render content/owner when using an
explicit bounded diagnostic on a disposable page. No-model diagnostics may exercise
the same body-mode, snapshot, text-input and persistence APIs without undo/redo or
status ink. Extra paragraphs are evidence to investigate, never permission to trim
or normalize the exact ownership comparison. Production persistence retains its
30-second bound and three equal fresh observations, with deadline checks before
and after expensive observations; a late third match cannot establish success.
Keep input/owner guards and failure propagation around every observation. Test the
actual shared wait with delayed matching observations, changed/error observations,
and fatal cancellation rather than testing a detached timeout predicate.

Measured ordinary indicator work used32 status observations across two leases.
Consolidate consecutive toolbar property decisions using one fresh classified
observation only when no input/checkpoint/callback intervenes. After every toolbar
mutation obtain a new observation; retain transition pre-input checks, durable
intent, bounded post-input verification, final style checks, and both sides of
cleanup checkpoints. Do not carry observations across independent operations or
use persisted preferences in place of actual UI. Verify fewer observations with
the production lease model and retain partial-failure/rollback regressions.

The nondefault diagnostic still spends about 49ms converting every 1404x1872
BGRA pixel and another 24ms normalizing each guard image, repeated across43
status observations. For the image-only modern RM2 path, fuse the exact existing
Nearest sampling and weighted BGRA luminance conversion: read the entire fresh
frame as before, but convert only the selected 768x1024 pixels. Keep the same f32
coordinate arithmetic, portrait layout, channel weights/rounding and alpha
behavior. Encoded native/detail-strip captures remain unchanged; legacy RM2 and
Paper Pro retain their existing conversion/normalization path.

This is a pixel computation change only. Preserve fresh PID, allocation discovery
and raw read on every attempt, clear both encoded buffers before all attempts,
and preserve malformed-length failures. No cached address/frame or EIO retry.
Compare fused output against the previous full native conversion plus the library
Nearest oracle across all channels/gray boundaries/random spatial patterns and
dimensions, including truncated/oversized frames. Reuse the existing exact-pixel
suite rather than a self-referential sampling test. Record fused timing as its own
phase, not as an additive comparison to nested historical conversion/resize
spans. Run native image equivalence and repeated full indicator measurements;
the microbenchmark alone cannot close the marker or full-workflow gates.

The first faster nondefault-primary case refused an unrecognized toolbar after
SelectFine. Before changing convergence policy, add opt-in diagnostic preservation
of the actual controls-refusal frame, its owner, last durable phase/sequence,
expected post-intent controls and observation timestamps. Content/identity guards
must run first. Preserve the first rejected frame for a given process/sequence;
never substitute a later capture or update the active baseline. Diagnostic write
failure must not replace the original safe refusal. Keep private raw frames and
the failed sample. An explicit offline diagnostic helper mode may enable the
existing debug-dump behavior; ordinary helpers and production CLI defaults stay
unchanged. Review this diagnostic before one bounded reproduction. Unknown
layouts remain fail-closed; any future pending state must be narrowly justified
by actual evidence and retain deadline, cancellation, content and owner checks.

In the implementation PR add a concise AGENTS/workflow rule applying this default to future Reader/Writer features, with public/manual equivalents and no optional integration prerequisite. REM35 audits new waits and verifies preservation across the integrated system. Simulator coverage must exercise immediate/delayed completion, missing/duplicate/out-of-order/stale signals, wrong page/session/operation and cancellation via actual production sequencing; assert no duplicated mutation or stale successful transition. The architecture amendment preceded implementation; the measurement checkpoint now includes the AGENTS principle, while tasks.md tracks remaining implementation and validation.

## Superseding current-tool direction (2026-09-23 UTC)

The user's September22 local-time steering, relayed by the read-only reviewer,
replaces the requirement to perfect simulated toolbar swaps. Drawing should
preserve the selected pen. Direct selection is acceptable only through a robust,
supported mechanism without menu-coordinate presses. Preserve the measured
capture/navigation improvements and the unresolved history work. Prior toolbar
failures and timings remain historical evidence; further elective reproduction
is paused. The diagnostic-only baseline b41b42d is preserved but not executed.

Implement a small current-tool status lease, without opening a menu to discover
properties. Fresh closed-toolbar pixels must positively identify a supported
pen class and visible color; saved xochitl preferences remain advisory. Width
is not observable in the closed toolbar, so eligibility must cover every width
and injected-pressure footprint for the admitted class, not assume Medium.
Initially investigate only Fineliner in either already-selected slot, using exact
native icon/color fixtures. Admit it only after a conservative maximum footprint
and eraser coverage are verified on the supported firmware. No Highlighter,
white/light ink, eraser, selection tool, unknown icon/layout or ambiguous color
is admitted by default. Broader tool support requires its own bounded evidence.

A supported mark must fit completely inside the reserved blank area including
stroke width and the physical eraser envelope. Retain owner/session/visit and
strict content checks, external-input cancellation, serialized input and durable
ink/cleanup intent before mutation. Cleanup uses the existing physical eraser
input, never a toolbar-selected eraser or broad page/rectangle wipe. Verify the
whole cleared footprint, neighboring ink and unchanged toolbar after cleanup.
Journal changes must preserve refusal of old unresolved records; never silently
migrate or discard recovery evidence. Do not claim a narrow centerline erasure
covers an unmeasured thick stroke. If no safe envelope is established, suppress
the optional mark before any input; do not experiment on user ink.

Suppression keeps normal Q&A/provider/keyboard output available, records a reason,
and does not claim visible feedback or successful indicator timing. Report this
coverage tradeoff explicitly. Both success triangles and failure X use the same
current-tool eligibility; persistent failure marks still require a safe footprint.
Generic line/bitmap helpers already preserve tool selection but need a guard on
any reachable automatic drawing path. Legacy unused draw_symbol and region-erase
helpers are not permission to add new automatic mutation. Text rendering/body
mode are keyboard operations and remain governed by existing content/history
contracts, not the pen-style gate. Future Writer drawing inherits this rule.

Before code: audit and preserve all actual call paths; review this plan. Then add
closed-toolbar positive/negative fixtures and model the actual current-tool lease,
including zero toolbar presses, either slot, stale preferences, unknown width,
changed owner/input, journal failures and cleanup bounds. Validate a bounded
no-model maximum-width/cleanup case on disposable material before admitting a
class in production. Native configuration during test setup is distinct from
production menu automation. Repeated native supported and suppressed cases must
show unchanged tool settings, safe neighbor ink and complete Q&A/errors; retain
provider-separated visible milestones and remaining latency/history gates.

Bounded API investigation: the official developer examples document separate Qt
Quick applications, not an observed xochitl pen-selection interface:
https://github.com/reMarkable/remarkable-developer-examples . The upstream inkling
xovi extension describes native tool switching through an injected extension:
https://github.com/nathanmarlor/inkling/blob/main/xovi-ext/README.md . This is not a
verified supported API on this tablet and would add an invasive dependency; do
not install it for this change. Prior denied D-Bus introspection only limits that
inspection. No robust supported direct selection mechanism has been established;
this is not a claim that none exists anywhere. Research stops here for this scope.

Further user steering (same session) makes this a product-wide architecture rule:
NO autonomous menu navigation in Reader, Writer or future features, including
read-only menu inspection. Simple left/right swipes remain explicitly allowed;
keyboard input is not banned. AGENTS and the development-testing delta record the
rule for contributors and REM35. Audit body-mode, navigation, failure and helper
call paths rather than assuming all touch input is a menu. Developer/manual setup
is clearly separated and announced; no diagnostic helper becomes a product bypass.

Further user research guidance qualifies the stopping point above: when a concrete
no-menu roadblock appears, inspect relevant current community repositories and
actual code/issues/releases for alternative mechanisms. This includes the present
current-tool observability/footprint roadblock. Record source links/revisions,
firmware compatibility and distinguish direct supported interfaces, native file
mechanisms, injected extensions and UI automation. No exhaustive survey or silent
invasive installation is implied. A prior failed D-Bus probe is not proof that a
community solution cannot exist; bring promising tradeoffs to review.

Clarification: a reliable community direct interface can satisfy the no-menu
principle; vendor-official status is not a prerequisite. The injected bridges are
candidates with unverified compatibility/deployment cost, not permanently excluded.

User decision after reviewing community findings: defer Qt/XOVI bridge reconsideration
until AFTER the full roadmap, and only if the user misses automatic marker
swapping. No installation, implementation, further bridge research or scheduled
investigation belongs to the current MVP. Retain source findings as optional
future context. Continue current-selected-pen/no-menu work with proven safe
eligibility and core Q&A available. Writer scope remains notes/notebooks to full
notes in the chosen backend, with optional refinement using the same plain Reader
Q&A interaction; no tablet rich-formatting requirement is introduced.

Current-tool candidate review follow-up: before/active and before/final toolbar
comparison in the thick calibration has exactly46 changed pixels bounded by
x20..40,y391..405 (PDF undo icon enabling). Only this small known region may be
excluded from the candidate active-toolbar comparison; no blanket outside-pen
chrome exemption. Notes-page undo position/state still needs separate evidence
before product admission. Current-tool cleanup additionally preserves a12-pixel
outer neighbor ring beyond the existing12-pixel blank clearance. This adds a
check; it does not establish the maximum eraser footprint or validate all widths.

Development-only `hardware_probe indicator-current-tool` constructs the candidate
current-tool backend explicitly; normal Reader construction still uses the old
path pending admission. The candidate retains its input observer between strokes
and provider/cadence intervals, checks buffered events before reuse, and discards
it only around its own physical pen/rubber injection. It rearms after release
including failed writes. Events occurring wholly inside that shared-source contact
window cannot be attributed; do not claim exclusive input ownership. Each pen line
(including failure-X lines) requires CurrentInk and fresh context before input;
each eraser path requires PendingCleanup with the original checkpoint/context,
unchanged toolbar/neighbor ring and the qualified viewport contract. Interim
cleanup may retain owned ink; final cleanup must prove the entire corner clear.
No baseline refresh, menu input, erasure broadening or retry is introduced.
Review source and cross-build before a bounded native candidate run; helper success
still requires positive mark/cleanup evidence, not just exit status.

Native candidate write-failure policy: always attempt observer rearming after a
pen/rubber write or release fails, then latch the failure. No subsequent ink,
erasure or automatic journal completion is permitted; deliberate recovery remains
required. Preserve the original injection error chain even when rearming also
fails. The shared native wrapper is exercised by injected partial-write/release
faults with a real V3 journal, including rearm failure and subsequent mutation
attempts. This supersedes any implication that a successful rearm alone permits
cleanup after an uncertain write.

Proposed allocation-lifetime recovery (pending exact plan review): apply only to
status observations and only an explicitly typed discovery-header EIO5 for which
a fresh maps sample no longer contains the candidate address. Do not classify
from error strings, whole-map inequality alone, raw-frame read errors, other errno,
ambiguous candidates or arbitrary observation failures. The existing discovery
scan still fails immediately; it must not skip a vanished candidate and accept
partial scan results. Preserve the complete original error chain and failure
address/range diagnostics.

At the status observation boundary, retain the initial native owner including
visit and process session. Permit at most one additional completely fresh image
capture after the typed failure, only after rechecking the same owner/session and
pending external input/cancellation. Discard first-attempt maps, addresses, raw
bytes, candidate list and cached outputs. Reacquire PID/maps/address from scratch;
then retain the existing final owner/session bracket and every lease pixel,
phase, geometry, neighbor and clear-corner predicate. Never repeat any input,
journal mutation, pen stroke or erase path. Use one monotonic500ms budget across
both capture attempts; check it before starting another attempt and after capture
before accepting pixels. Synchronous OS reads cannot be preempted by this budget;
late completion must be rejected, not published as a successful observation.
If revalidation, retry or deadline fails, preserve both error contexts, latch the
failure through existing callers and retain the recovery journal. Normal unrelated
capture paths retain current fail-closed behavior. No menu, cached frame, baseline
refresh, retry loop, model call or historical-error claim is introduced.

Test through the shared observation recovery mechanism: vanished discovery
candidate followed by a fresh valid capture; same errno with still-mapped address;
other errno; raw-frame EIO; persistent failure; changed owner/visit/process session;
cancellation and external input; ambiguity; deadline expiry before retry and late
success; cache clearing and at most two reads with zero input operations. Preserve
an exact sampled-region regression and distinguish synthetic faults from native
proof. Review implementation and ARM build before any further native attempt.

Recovery implementation scope refinement: enabled only in explicit current-tool
candidate observations when an existing input observer is already present.
Prelease calls without that observer, legacy status, navigation and other capture
paths keep immediate failure behavior. The observer is never created or reset to
qualify recovery. The whole one-shot status observation (fresh owner, image,
metadata and final owner/session) is repeated once; the initial recovery owner
remains fixed and input is checked before/after owner reads. All failed participating
observations latch cancellation before returning, so acquisition rollback cannot
continue into journal completion after an uncertain observation. Each capture
still clears encoded caches before discovery and owns/discards its raw buffers.
The500ms bound includes metadata and revalidation; ordinary499ms synthetic capture
passes,500ms and late second completion refuse. Four shared recovery tests plus
one typed-cause/partial-scan regression are modeled evidence, not native recovery
proof. Existing190-test full suite passed on ddbd8dd before this recovery change.

Cleanup measurement follow-up: the successful4280 ten-path erase/verify parent
contains about2.12s in ten status.observe spans, but the remaining time is not
separately attributable yet. Add content-free status.path.guard/inject/rearm and
status.checkpoint spans without changing input timing or geometry. The next bounded
secondary Black/Thick case should combine missing-slot coverage with measured
cleanup attribution. Nested spans remain non-additive. Retain100ms final settling
wait as an inventoried completion-work item, not permission to delete it blindly.
Potential future exact-endpoint joining requires a separate reviewed proposal,
same ordered owned segments, no bridges or broad region erasure, bounded contact
exposure and native corner/neighbor proof. No joining is implemented here.

Nested failure propagation follow-up: WaitCancellation.record retains an incoming
error chain even if an inner observation already latched cancellation. A successful
result still refuses when the latch was previously set; cancellation is monotonic.
The regression checks preserved EIO5/context and no revival after a later Ok.

Second-level guard instrumentation separates existing status.capture.owner_guard,
status.input_guard, status.active.verify_pixels and status.cleanup.verify_pixels.
These scopes remain nested and content-free; their names describe call scopes,
not mutually exclusive workload categories. Use the next missing-width native
case to attribute guard overhead before changing predicates or path grouping.
No capture cache, tolerance/mask, ownership check, eraser geometry or pacing change.

Proposed next concrete optimization (pending review, not implemented): preserve
native input read descriptors across each explicitly owned pen/rubber contact
window instead of dropping/reopening the whole observer. Identify the injected
source by the actual pen writer descriptor's device/inode identity, matched to the
unchanged inventoried source, never by a broad device-name exclusion. Before the
window, preserve the existing fresh owner/pixel/input checks and require quiescent
pending events. During the window the observer cannot authorize other operations.
After release, drain only the identified source's expected pen/rubber event classes
with bounded count/deadline, reject dropped/malformed/unexpected events and held
keys, then restore ordinary polling. Keep every other source's buffered events;
a touch/key gesture during injection must still invalidate ownership even if it
ended before release. Recheck inventory and kernel contact/key state. No reset of
cancellation, baseline, page identity, other-source events or observer clock.
Always finish the owned window after attempted release including write failure;
shared failure boundary still latches and retains journal. Events wholly inside
the same physical pen's injection window remain unattributable, as before; do not
claim exclusive stylus ownership or expand the window. No path joining, new erase
segments, pacing change, menu action or background/deferred close worker.

Require deterministic tests for pending input before entry, unrelated released
contacts during injection, unexpected pen events, SYN_DROPPED, held contacts/keys,
inventory/descriptor mismatch, drain overflow/deadline, failed writes/releases,
failed rearming and no later mutation or journal completion. Keep existing observer
behavior for noncandidate workflows. Review exact source and ARM build before one
bounded native case; measure identical full cleanup and neighbor/footprint/tool/
journal checks. Native delivery of injected events to retained descriptors must be
verified, not assumed. If the observer cannot establish the contracted window,
fail closed without falling back to a blind drain/retry. This targets the measured
teardown interval rather than weakening fresh guards.

Retained-descriptor candidate implementation: duplicate each live writer/reader
fd only for File::metadata/fstat, match character-device inode/rdev and unchanged
inventory; do not identify ownership by a mutable path alone. The existing guard
is immediately followed by begin_owned_pen, which processes pending input before
marking the window. Ordinary poll refuses while that window is open. After the
release attempt, finish drains only the exact source with an8192-event total cap,
50ms drain deadline and one-second total contact-window bound. Only injected
coordinate/pressure/distance and pen/rubber/touch-key codes/values are accepted,
with complete SYN frames, mutually exclusive tools, pressure/contact consistency
and a final released state. Dropped, malformed or unexpected events fail closed.
Fresh kernel touch/key snapshots and repeated inventory/fd checks precede ordinary
polling of every source. Buffered other-source gestures (including completed taps)
and late pen events invalidate ownership rather than being discarded. No observer
clock/contact reducer/cancellation reset. The shared input-failure wrapper always
attempts finish after write/release failure and prevents later input/journal finish.

Four shared-policy regressions cover valid split frames, identity/snapshot/external
input refusal, malformed/dropped/partial/held events, count/deadline limits, and
combined write/rearm failure with preserved OS chain and no later mutation. The
native adapter's actual kernel queue and fd behavior still require exact source
review, ARM compilation and bounded hardware validation. These tests do not claim
to inject real kernel hotplug, held contacts or persistence failure. Earlier real
journal failure regressions remain required. No automatic fallback to descriptor
replacement, generic drains or broader erasure is present.

Linux production-observer regression follow-up: extract the existing event feed
and final frame/timer processing without changing decoding rules. Test-only replay
replaces OS queue reads, descriptor checks and released-state snapshots, then runs
the actual InputObserver.finish_owned_pen / WindowAdapter / poll decoder / contact
reducer. Model completed touch down/up and key down/up queued while owned pen input
is pending, plus late pen activity. A quiet positive control must pass; each external
case must latch observer loss and shared cancellation, refuse later injection, and
retain the exact real current-ink recovery journal. This complements native positive
event-delivery evidence; it does not establish kernel hotplug, real contact concurrency,
or syscall fault handling. Run this Linux-only test under the ARM target emulator;
Windows-only test success cannot validate this path.

The completed-external-input regression passed on ARM Linux under the target
emulator (1m43 build, test0.39s). It covers completed touch/key and late pen queues,
a quiet success control, sticky loss/cancellation and byte-identical retained
recovery journal. The syscall reads are modeled; this is not native concurrency
or physical-contact proof. Host strict clippy/fmt and OpenSpec validation passed.

Next bounded native coverage uses the existing positive current-tool cycle for
secondary Thin/Medium. The failure-current-tool diagnostic selects that same
candidate constructor and calls the unchanged production draw_failure(code),
retaining a before image. Failure X/code marks intentionally persist as before;
they are not activity paths and must not be reported as automatically erased.
Developer cleanup uses only the exact recorded X and selected-code paths after
fresh owner/image checks. Each case stops at first semantic failure, retains any
journal, verifies neighboring strokes/actual tool, and restores original state.
No normal-constructor admission yet. All131 Linux ARM library tests passed under
the emulator at1530075 (65.14s); independent review found no source blocker.

Durable-erasure finding and next bounded diagnostic plan (before implementation):
The secondaryMedium sentinel block audit shows the same top-line CRDT item(1,1857),
unchanged points/properties and deleted_length0 in post-cycle, post-failure,
immediate post-removal, post-close and later snapshots. It disappears only after
the visually confirmed reopened line receives one deliberate same-path erase.
This is stronger than simple delayed observation: that line was visible again
on reopen. It does not establish the runtime cause or prove product indicators
recur. Existing rmscene tree comparisons omit unsupported/tombstone interpretation;
retain raw blocks and visual evidence rather than treating parser count as universal
native truth. Original recognized lines remained unchanged.

Manual e4 erase-strokes and candidate use the same Pen.erase_path_screen. Manual
paths lack intervening fresh framebuffer/owner/input guards; nextpath pen_up still
introduces10ms, so this is not a zero-gap claim. Candidate guards add measured
observation time but are not themselves proof of durable application erasure.
No repeat-erasure policy, minimum-delay guess or geometry change is authorized
by this finding. Preserve it alongside the distinct unresolved history persistence
and extra-newline findings; no common cause has been established.

Bounded community check: Ghostwriter currentHEAD equals the already pinned
5c2f2560328bff536f7916dca540dc40efb48020. Its
[pen source](https://github.com/awwaiid/ghostwriter/blob/5c2f2560328bff536f7916dca540dc40efb48020/src/pen.rs)
contains hover/contact/release drawing but no rubber-erasure completion protocol;
no eraser/stroke-titled issue in the inspected100-item issue listing supplied a
verified solution. No Qt/XOVI research or dependency work resumed.

Next diagnostic-only amendment captures bounded native page bytes at initial,
active-mark and final stages of the existing current-tool cycle, each with fresh
owner/session bracketing and monotonic phase logging. Save private artifacts,
never change native bytes, page baseline, input observer or normal constructor.
Native active snapshots must actually contain owned marker ink to support a
persisted-ink-to-persisted-clear claim; unchanged old bytes are not success.
After a successful guarded cycle, deliberately close/reopen the disposable page,
verify owner, visual clearance and native original-content preservation, and keep
all intermediate snapshots. This is test evidence, not a proposed product navigation
or forced-flush behavior. If active ink is not yet persisted, record that limitation
and design a bounded observation predicate; do not silently sleep/repeat mutations.
Before broader native coverage/admission, distinguish manual sentinel failure from
production erasure and establish durable cleanup evidence on the tested path.

Diagnostic review correction: Linux examples cannot access the library-private
measurement API. Snapshot timing uses local Instant and fixed-label debug begin/end
instead; no internal API is exposed. Final native snapshot occurs after successful
normal cleanup and journal retirement. Failure at that point is a diagnostic evidence
failure, not a claim that the already-retired journal remains. Active-stage failure
still stops before automatic cleanup and preserves its prior recovery phase.

Next bounded diagnostic proposal after stale active snapshot (no code yet): while
leaving the already-drawn marker untouched, observe native file bytes until a change
from the pre-draw snapshot, with30s monotonic budget and500ms test-only poll cadence.
This is a data-change signal, not proof of owned ink; inspect saved IDs/geometry
offline before claiming persisted-active-to-erased success. No extra drawing or
forced save/navigation. Use a separate read-only InputObserver throughout this
idle diagnostic wait (no reset of the retained production observer), reject input
before/after fresh owner/session-bracketed reads, and check deadline before/after
observations and candidate acceptance. Preserve the first changed snapshot privately
and then use unchanged guarded production cleanup. Unchanged samples need not copy
native bytes repeatedly; log content-free timing/count and bound storage. Timeout,
input, owner or read failure retains active phase/journal and stops without automatic
cleanup, as existing native-evidence mode does. Final/reopen saved native identity
comparison and clear pixels remain separate acceptance checks. Do not add this wait
to normal Reader or interpret elapsed time/changed bytes alone as successful erasure.

Review revision: raw byte change is not an acceptance predicate. In the explicit
native-evidence diagnostic only, keep the drawn marker untouched for a maximum30s
monotonic observation window, paced500ms. Preserve each distinct changed native
snapshot using create-new private files (maximum16versions,64MiB cumulative;
8MiB per read); publish its numbered ready record only after the snapshot closes.
An external, bounded host diagnostic may inspect these immutable snapshots using
the existing rmscene parser. It must retain every original recognized stroke ID
and property and verify the added marker's expected geometry/count and Black Fine
style in the known status region, not merely a count or changed hash. Parser
ambiguity, extra/unexpected records or baseline changes forbid acknowledgement.
Unknown native records remain an explicit evidence limit. No new on-tablet parser.

Only an atomic diagnostic acknowledgement naming the exact validated snapshot
number permits continuation. Before accepting it, the device must compare fresh
two-equal bounded page reads with that snapshot, recheck fixed owner/session,
separate continuous read-only InputObserver and the same deadline. A stale/invalid
acknowledgement fails closed. The host's acknowledgement is test evidence, never
production IPC or an automatic runtime policy. Persist the validation report and
snapshot hash on the host. The existing production observer is never reset or
replaced; it also retains all events for the next unchanged guarded cleanup.
No acknowledgement, timeout, input, read/owner failure or storage-cap exhaustion
returns the original failure before automatic cleanup, retaining active journal.
The host has a bounded watchdog, and cannot acknowledge after its own deadline;
the device's deadline is authoritative. No drawing retry, forced save, active-page
close or marker redraw. Successful acknowledgement leads to the unchanged normal
cleanup, saved-final check and separate deliberate developer close/reopen audit.
Test metadata-only changes, unexpected lines, stale acknowledgement, timeout and
input/owner cancellation without claiming synthetic fixtures are hardware proof.

Diagnostic implementation details: SHA-256 is an example-only dev dependency;
normal installed runtime dependencies/behavior are unchanged. Host parser runs in
a separate process with at most5s/remaining-host-budget per operation and60s total
startup-inclusive budget; device30s deadline remains authoritative. Numbered ready
records and expected geometry publish atomically after private artifact closure.
Host ACK carries unique create-new run directory, version and SHA-256. Device
fresh-byte equality and read-only owner/input checks gate acceptance.

The fixed native transform is calibrated from this disposable viewport's left/right
sentinels, checked against eight independent top-sentinel/selection-X endpoints
(maximum residual0.681 native units). The fixed1.0-native-unit tolerance is under0.4
screen pixels and is not relaxed on failure. Actual helper Stroke::points exports
nine triangle edges and the final auxiliary circle. Predicate requires ten unique
ordered bidirectional trajectory matches, exact Black Fine Medium recognized style,
all baseline line IDs/properties and opaque non-PageInfo blocks unchanged. Existing
line-deletion blocks are compared opaquely; new/changed ones refuse acknowledgement.
Unknown semantics remain a limitation. Six synthetic regressions pass; retained
a511 active bytes refuse for missing ten marker lines, and its later saved bytes
refuse due to changed deletion records. Neither old snapshot becomes active proof.

Diagnostic correction after bc692a5: keep nine edge draws and six auxiliary ticks;
export exact per-unique-path multiplicities [1,1,1,1,1,1,1,1,1,6] using the same
auxiliary-tick constant as the helper loop. Require exactly15 added lines with each
path's exact count, rejecting missing/extra/replaced circles. Ten unique eraser paths
are distinct from fifteen physical draws. Seven synthetic regressions now cover
this distinction. Native geometry remains refused at its existing strict bound;
no calibration relaxation or further native execution is included in this fix.
Investigate the full emitted integer-coordinate and saved-curve interpretation
pipeline offline, retain independent cases/negatives, and review an evidence-backed
predicate before another native test. Never use this candidate as its own expected
fixture. Product erasure and broader admission remain unverified.

Bounded offline reconstruction after count fix: exact f32 virtual-to-input scaling
and integer follow-path interpolation reconstructed from bc692a5 source gives
maximum waypoint quantization0.1465 native units. Persisted circle vs reconstructed
commanded trajectory still differs1.283/1.574 native units bidirectionally; this does
not establish that integer input rounding caused the discrepancy. This is source
reconstruction, not recorded kernel events. No geometry bound changed.
Current rmc revision da87813a31496d156ca6ea8a27bf5128670fb45a renders stored points
as a [polyline](https://github.com/ricklupton/rmc/blob/da87813a31496d156ca6ea8a27bf5128670fb45a/src/rmc/exporters/svg.py);
its [Fineliner width](https://github.com/ricklupton/rmc/blob/da87813a31496d156ca6ea8a27bf5128670fb45a/src/rmc/exporters/writing_tools.py)
heuristic is not a vendor guarantee of xochitl smoothing or native eraser behavior.
No direct supported curve-completion mechanism or validated smoothing bound was
established by this targeted source inspection. No extension/Qt work resumed.

Reviewed next diagnostic predicate amendment (before implementation): define only
circle geometry as footprint-equivalent within half of the independently measured
2-output-pixel Medium sentinel width. Use1.0 output pixel times the smaller fixed
native/output scale: min(2.5122531797827743,2.5302505493164062)=2.5122531797827743
native units. The pixel grid has up to half-pixel boundary uncertainty; this bound
is a deliberately stated output-footprint comparison, not a measurement of exact
physical pen radius or a vendor guarantee. Do not enlarge it to absorb that
uncertainty or tune it against the rejected circle. Nine edge paths retain1.0 native
unit. Keep exact Black Fine Medium style, six circle repetitions, unique ordered
bidirectional path coverage/endpoints, owned ROI, original full blocks and opaque
records, and all ownership/input/deadline checks. Test circle translations, radial
expansion and detours inside/outside the defined bound to disclose what equivalence
accepts. Correctly recognized active ink alone is not erasure success: after the
single reviewed native cycle, require ALL15 accepted added IDs gone, every original
ID/property preserved, then fresh owner/reopened visual clearance. Unavailable saved
final proof is inconclusive; no repeated drawing loop. No more smoothing research,
new product waits or generalized diagnostic framework is part of this amendment.

### Normal Reader selected-tool integration plan

After the limited durable-cleanup evidence, remove the development-only backend
mode split: normal RealDevice construction and the explicit probe share the same
V3 current-tool path, retained input observer, guarded pen windows and typed capture
recovery. Eliminate the backend boolean escape to V2 acquisition/unguarded strokes;
retain historical V2 parsing/tests for unresolved-record compatibility, but make
native StyleIo::press refuse without emitting touch input. No normal path may
select or inspect tools through menus. Existing unresolved journals still block
startup, and post-input failures still retain their evidence.

Admission remains narrow: actual closed Black Fineliner in either calibrated slot,
RM2 verified firmware contract, validated footprint/viewport, blank owned corner,
and the native PDF annotation toolbar band (eraser/selection/layers in the measured
positions). Compare the existing native toolbar fixture's rows185..380, outside
selected pen tiles and the changing undo icon. This rejects notes-page toolbars
(which include an extra text tool), open/unknown layouts and unsuitable tools
BEFORE journal creation/pen input. Notes/blank/unknown layout feedback remains
explicitly unavailable while core Q&A remains available; do not call this positive
feedback coverage. Existing actual width-range calibration still bounds any admitted
Black Fineliner width. No diagnostic persistence wait/ACK enters the product.

Propose removing the automatic bottom-center trigger-dismiss tap from the backend/
workflow and simulator as a simplification, subject to native trigger/overlay tests.
A simple non-menu tap remains authorized if evidence establishes it is necessary.
Keep simple page swipes and keyboard input as authorized.
Verify the no-tap trigger path natively before full workflow admission; no new menu
fallback if an overlay is present. Keep all current recovery/content guards.

Simulator impact: retain shared native-pixel eligibility/lease tests, add explicit
modeled status capability per page (calibrated Black Fine PDF, unsuitable tool,
unknown tool, unverified layout), and route unsupported states through the same
pre-input suppression outcome while asserting exact Q&A output and no mark/erase
operations. Preserve existing status-failure/cancellation scenarios and zero tool
mutation assumptions. Add actual known notes-toolbar and perturbed-layout refusal
regressions to the shared eligibility predicate. Modeled capability is a declared
input, not hardware/tool recognition proof. Add a trigger trace regression proving
no synthetic dismiss tap and preserve gesture/release/cancellation semantics.

Before release: exact source review, appropriate host/Linux tests and both target
builds, remaining native negative/error/notes/trigger/full-Q&A and history gates,
then normal runtime/latency checks. This integration is development work, not a
claim that remaining roadmap gates are complete or authorization to install an
unreviewed runtime. History persistence/newline investigation remains separate.

Admission refusal for an unsupported layout is distinct from safety failure: owner,
input or capture errors propagate and stop the operation; they are never converted
to optional-feedback suppression. The pixel band recognizes only the calibrated
layout, not semantic PDF type, and conservatively refuses other configurations.

### Controlled history provenance investigation

Before changing history comparison or retrying a paid Q&A, use the existing
no-model append diagnostic and bounded read-only snapshots of the disposable
notes page. First preserve closed-page bytes, open it without typing, and record
fresh owner-bracketed bytes/parsed text/full scene blocks through a30s monotonic
observation window. A changed PageInfo first field plus three identical fresh
snapshots is a measured transition; unchanged bytes at deadline are inconclusive,
not proof that no delayed transition exists. Repeat one close/open no-edit control
only to distinguish an opening-related counter from text editing. Preserve every
changed version, exact owner/visit/session and before/after screenshots.

If the no-edit controls retain exact text, root layout and all other scene blocks,
run at most one existing append_probe on that controlled page with one bounded
known ASCII Q&A. Save its in-process before-body, after-body, before-render,
immediate-after-render and final snapshots/seals, exact requested/expected text,
and complete errors. Compare the changed counter separately from every other
field; do not infer safety from the parser's loads_count name alone. Any extra
newlines, changed ink/layout/owner or unexplained native records remain a failure
requiring diagnosis. No product normalization, cursor repositioning, forced save,
fixed persistence delay or paid retry follows automatically from this evidence.
A proposed production fix must receive its own evidence-backed plan/review and
regression fixtures before implementation. Manual navigation is diagnostic setup
only and original document/tools/service are restored at batch end.

### Conditional outside-overlay dismissal after the no-tap failure

Native50cb909 accepted Reader release but left the known lower-left overflow
panel open. Supersede unconditional tap removal with a bounded conditional
outside-panel dismissal; no menu item is selected, and no other menu navigation
is permitted. Preserve the actual open/closed pair as regression fixtures. Qualify
the measured RM2 firmware contract and known panel pixels (x61..279/y655..1023),
selected tool tiles and surrounding toolbar. A positively closed toolbar/seam
skips input; an unknown or partial panel refuses. Do not claim recognition of all
popovers, languages, firmware or tool-dependent menu variants. Expand coverage
only from independent real fixtures, preserving unsupported-tool core-Q&A gates.

Before the single permitted tap384,1023, pin document/page/visit/session, bounded
native page bytes and a fresh image, then prove all input sources released. The
point lies outside the qualified panel and menu items. Retain the input observer
across a narrowly owned touch window using actual reader/writer descriptor identity
and unchanged device inventory. Use a separate touch-event policy, not the pen
whitelist: at most one slot0 contact at the exact native point, complete down/up
frames, positive delivery, no extra contact/coordinate/key or dropped event, and
released kernel slots/keys. Keep other-source pending events and reject them in
the ordinary final poll. Bound the physical contact to the existing100ms candidate
exception, owned window1s, drain50ms/8192events. Always attempt release and finish
observation after write failure; any write/release/rearm failure is sticky. A real
finger coinciding with the identical owned stream cannot be attributed uniquely;
state this limitation instead of claiming exclusive physical ownership.

After release, use fresh read-only observations with a5s monotonic deadline and
50ms pacing, no repeated tap and no fixed post-tap settling delay. Require known
panel absent, same owner/native page bytes and unchanged selected tool/exposed
content outside x0..279/y655..1023. Pixels hidden by the former panel have no visual
baseline; native byte equality supplies a separate bounded content check, not
universal format semantics. Page-byte changes, external input, changed exposed
content or unknown controls fail immediately; stable known-open pixels may wait
until deadline. No new baseline, blind dismissal, menu fallback or draw/erase.

Wire both production Reader trigger dispatch paths through this operation and
restore a release-observation measurement marker separate from dismissal duration.
Any failure latches Workflow input failure and prevents later Q&A mutations. Keep
historical V2 menu code unreachable. Extend the shared simulator with known-open,
closed and unknown trigger-overlay inputs, once-only conditional tap traces,
postcondition/cancellation faults and exact core answer preservation. Native-pixel
fixtures test the real classifier; modeled overlays do not prove native behavior.
Test late/stale frames, owner/content/tool changes, partial/malformed touch frames,
completed other-source input, lost devices and write/release/rearm failures before
reviewed native validation. No runtime replacement or broad admission until tests,
builds, source review and the focused native trigger/capture/navigation gates pass.
