# Local Reader simulator

Run the real Reader workflow on Windows, Linux or macOS without SSH, tablet input
devices or an API key:

```sh
cargo run -- --simulate docs/simulator/scenarios/blank-answer.json
cargo run -- --simulate docs/simulator/scenarios/failed-return.json
cargo run -- --simulate docs/simulator/scenarios/history-undo-redo.json
cargo test --test simulator
```

Each scenario writes `page-0.png`, `page-1.png`, etc. and `report.json` beneath its
configured output directory (bundled scenarios use `target/simulator/<name>`).
The report contains exact text, newly emitted X counts, unchanged-page results,
active page, virtual elapsed milliseconds, model-call counts and ordered actions.
Expected-result mismatches exit nonzero **after** saving results. Expected injected
failures can pass when the recorded behavior matches the assertions.

The scenarios above use **scripted and offline** replies by default. The application still builds its
real prompts and images, parses proposals, independently compares transcriptions,
classifies screenshots and executes its real recovery/rendering decisions. This
tests application behavior, not whether a model can read the handwriting. The
scripted engine validates that image content is decodable and traces content counts.
It does not connect to OpenAI even when credentials exist in the environment.

Explicit live provider execution is available through the [local development setup](local-development.md). It requires a live scenario and credentials; the deterministic defaults below remain offline.

## Scenario format

Start from a maintained JSON file in [simulator/scenarios](simulator/scenarios).
Unknown fields and invalid states fail before execution. Page indices are zero
based; operation call numbers are one based and span the whole scenario.

| Field | Meaning |
|---|---|
| `llm` | Defaults to scripted/offline. Explicit live configuration is documented in local development setup. |
| `name`, `mode` | Nonempty name; currently only `reader` executes. |
| `pages` | Ordered pages. `{}` is blank. `image` loads a 768x1024 PNG; optional `text` and `strokes` add deterministic text or existing JSON pen paths. |
| `active_page` | Starting page, default zero. |
| `iterations` | 1–100 bounded Reader iterations. Optional `page` models the user selecting a source between iterations. `wait_for_trigger` defaults false. |
| `trigger_corner` | UR/UL/LR/LL, default LL. |
| `gestures` | One sequence per trigger wait. Frames contain increasing `at_ms` (within 60 seconds), `contact`, `x`, `y`. Coordinates represent complete slot-zero frames. A final active contact can reach its deadline without further movement. |
| `replies` | Ordered objects containing exactly one `text` or `error`. Proposal and independent reading consume separate entries. Every declared reply must be consumed. |
| `faults` | Objects with `operation`, `call`, `effect`. Every fault must be reached. |
| `header_image` | Optional initial cached header image, useful for a preexisting answer page. Normal blank-page rendering updates the in-memory cache. |
| `output` | Directory for results. Asset and output paths resolve relative to the scenario file, independently of the launch directory. |
| `expect` | Optional `active_page`, `model_calls`, exact `text` and `text_contains` substring lists by page, `x_count` by page, `unchanged_pages`, exact operation counts and ordered error substrings. Errors default to none. |

Fault operations are `capture`, `next`, `previous`, `text`, `body`, `line`, `trigger`
and `header_save`. `error` injects an operation error; `no_move` applies only to
navigation; `stale` returns the preceding capture; `corrupt` returns invalid PNG
data. Stale/corrupt effects apply only to capture. Recovery/render errors caught by
the production orchestrator remain caught: their injected effects are in the trace,
while the top-level `errors` list only contains errors escaping an iteration.

Examples cover blank output, append, absent/illegible question, disagreement,
occupied successor, return failures, end page, forward no-motion/staleness, capture
and model errors, cache failure, corrupt classification/header captures, output
failure, stationary holds, interrupted holds and short taps. Assertions include
source/other-page preservation and forbidden extra calls, not just successful exit.

## Fidelity and hardware boundary

The model is grounded in RM2 3.28.0.172 portrait observations documented in PR10,
PR12 (REM-13) and PR13 (REM-10). Existing repository screenshots provide real
technical PDF and handwriting inputs; existing pen-path fixtures can be overlaid.

| Behavior | Simulator treatment |
|---|---|
| Overview | 768x1024, same production masks and thresholds: source 0.999, blank/header 0.998. |
| Hold | Same timer and 68-pixel corner predicate; short/released/out-of-zone contact resets it, stationary hold triggers at two seconds. |
| Navigation | Ordered pages, no implicit page at document end, 700 ms modeled swipe/transition plus production 800 ms wait. |
| Classification/header | Same pixel comparison and header crop; local in-memory cache substitutes for `/var/cache/reader-buddy`. |
| Rendering | Deterministic bitmap lettering and line strokes; exact case/text in JSON, uppercase approximate lettering in PNG. Overflow is clipped visually but retained in the report. |
| Model detail | Three overlapping strips from the simulated overview; these cannot replace full native-resolution vision evidence. |
| Failures | Explicit deterministic faults; they do not establish physical input failure rates or framebuffer reliability. |

Virtual time records requested waits, not CPU runtime, API latency or physical
display latency. History sessions feed complete virtual contact frames to the
shared multi-contact reducer; separate decoder tests cover raw slot framing.
The simulator does not emulate kernel device discovery/raw coordinate scaling,
kernel input injection, xochitl typography/cursor state, native file serialization,
paper orientation, reboot, framebuffer allocations or human handwriting quality.
It cannot prove hardware compatibility or catch every native output/layout defect.
The real adapter retains existing Screenshot/Touch/Pen/Keyboard operations and
normal Linux cache behavior. Targeted hardware checks remain required for those
areas, especially after changing the shared boundary.

## Q&A history sessions

Each iteration may include `actions` after its Q&A completes. A `hold` action has
`frames`, each with increasing `at_ms` and a `contacts` array of `slot`, `tracking`,
`x`, `y`. Empty contacts release all fingers. Virtual timer ticks qualify stationary
two-/four-contact holds; the shared policy executes only after full release.
`page` selects a page, `edit` appends manual text, and `input_lost` or `restart`
invalidate ownership. A new iteration always forgets the previous transaction.
The report includes `history` state, exact page text and ordered history events;
`expect.history` can assert `empty`, `applied` or `undone`.

Faults `history_snapshot: stale` retain an old stable snapshot and prevent arming;
`history_snapshot: wrong_page` models the observed case where stable last-opened
metadata refers to a different document than the visible one and cannot arm history.
`history_snapshot: lag` models an11-second delay followed by the complete expected text,
matching the order of observed RM2 persistence delays. This remains modeled time.
That delay is a deterministic test value, not a measured persistence bound.
`history_mutation: partial` retains the actual partial operation and reports an
error; subsequent undo/redo cannot compensate. `error` and `corrupt` also exercise
observation/mutation failure. All declared faults must be reached.

The simulator uses the production ownership state and Q&A registration boundary.
Its native-edit substitute selects a suffix from an assumed valid insertion
cursor and retains one deletion entry. It does not prove real cursor positioning,
xochitl's arbitrary Undo grouping, short-tap native Undo, native typography,
input-device attribution or file persistence. Captured RMv6 fixtures separately
replay exact text/styles, visible-ink records and opaque metadata preservation.
Neither replay nor scripted contact frames prove physical-finger or model-vision
behavior. Keep corresponding native tests and discovered failures in the same PR.
The overview-tap regression preserves another distinction: `LastOpen` can remain
set while the page overview is shown, so a page-UUID comparison cannot replace
continuous input invalidation.

The REM-22 hardware check exposed a restart-dependent allocation case: Linux can
merge adjacent anonymous mappings, placing the BGRA allocation header inside a VMA.
Real capture now checks page-aligned headers whose entire expected allocation fits
within eligible writable anonymous mappings. It still requires exactly one valid
32-bit mmap header and refuses missing or ambiguous matches. No live address or
historical fixed-offset fallback is used. Pure tests cover bounds, permissions,
alignment, merged maps, invalid headers and ambiguity; retrieved screenshots after
fresh UI restarts are the separate evidence for actual image correctness.

Screenshot identity remains heuristic: `stale-forward` intentionally shows how a
stale source capture can leave the X on the successor. The simulator records this
limitation; it does not pretend page identity became native or reliable.

## Later product coverage

`writer` and `combined` modes fail explicitly today. Production Writer and shared
gesture arbitration do not exist yet, and native insertion is also deferred. Do
not treat this Reader suite as full-platform acceptance. REM-23 must add real Writer
and combined scenarios through these interfaces; REM-25 must add insertion and
native-document preservation cases; REM-17 must validate final gesture routing,
combined workflows and remaining 1.0 acceptance on simulator and real hardware.
REM-28 provisions local credentials for later explicitly selected live-model work.


## Highlight recognition checks

REM-16 fixtures contain native RM2 highlighter marks, rather than drawn circle
substitutes. `highlighted-g`, `question-no-selection` and `highlight-no-question`
are deterministic routing cases; their scripted replies do not prove vision.
The corresponding files in `simulator/live/`, plus `highlighted-cursive.json`,
make explicit provider calls. Existing `live/reader.json` remains the circled
compatibility case.

A narrow yellow-highlighted G shorthand fixture was conservatively declined in
the local overview-based run; the same concept succeeded on hardware with native
detail strips. The local cursive highlighted fixture succeeded. A hardware cursive
attempt read "flat" in the proposal but "that" independently and correctly refused
output; one unchanged repeat agreed and appended successfully. These outcomes
are retained as evidence of model variability and image-fidelity limits, not
universal recognition guarantees. Do not loosen independent agreement to force
an ambiguous question through. The live G scenario states the intended acceptance
and can fail on a conservative decline; its expectations require the requested
uncertainty, not an unrelated requirement to repeat the central G value.

The highlighted illegible-question fixture also declines in separate live local and native checks; the deterministic case checks the corresponding no-navigation/no-answer route.

## Progress lifecycle coverage

The shared workflow now uses a guarded 50x50 status area. Simulator reports include `indicator_visible` per page and `statuscircle`, `statusclear` and `status_suppressed` trace events. Native pen input is serialized; the simulator rejects capture/navigation/typing while a temporary circle remains. This makes cleanup ordering observable rather than inferring it only from a final white corner.

Tests cover accepted output on both pages, preexisting corner strokes with continued answering, partial circle failures, cleanup failure blocking output/navigation, persistent failure across a page change, real local-HTTP delay callbacks, timeout and callback-error completion. Scripted ticks are deterministic and do not establish real-time display behavior. Native cleanup/tool behavior needs the separate hardware acceptance run.

Some older negative fixtures already contain corner marks. Their updated expectations preserve those marks and suppress a new X. Capture/navigation failures with unknown corner eligibility and possible partial typing also suppress the X. These fixtures still assert the original no-answer and bounded-navigation behavior; the preservation fallback is not recognition success. The live highlighted-illegible fixture likewise retains its preexisting X without adding a second one.

REM-8 native acceptance used a disposable copy of the Gundlach technical paper: blank Q&A, cursive append, absent/illegible declines, occupied-successor single return, and a real 90-second timeout with a loopback endpoint/dummy key. The final geometry build repeated Fineliner/Highlighter circle cleanup and live rejection; both active circles fit the 50x50 box, cleanup had zero changed ROI pixels and neighboring native ink survived. The prior sparse-path and highlighter-bound failures were retained in PR evidence. Model tests use synthetic pen input and do not establish universal human-handwriting recognition.

The history_snapshot unavailable fault preserves the observed post-restart case where a document is visible but LastOpen is empty. Normal rendering continues, while that transaction never gains undo ownership even if identity later becomes available. Reopening and a new successful iteration may establish a new transaction.

## Coordinate tags and native history limits

REM-11 scripted circle/highlight responses pass a selected-content center in the
full 768 by 1024 overview. Production parsing rejects invalid centers before the
independent model call or navigation. Exact expectations cover compact normalized
tags, and shared history tests preserve earlier untagged content. Native tagged
first-Q&A applied/deleted/restored snapshots additionally replay complete text,
styles, root layout and scene-record preservation through the production parser.

The live RM2 check also found an ordinary append where a qualifying undo did not
act. The same symptom occurred with the prior merged REM-15 runtime; a comparison
with answer-page activity suppressed by existing corner ink allowed tagged append
undo/redo. This is an observed availability limitation, not a relaxation of native
preservation. Later scene records differed from an earlier completed snapshot, but
the exact live pre-typing snapshot was not captured, so activity persistence is
only a hypothesis. The changed-scene fixture asserts conservative refusal across
those snapshots; it does not reproduce or establish the live failure's cause.

Model locations are approximate. Repeated native readings of the same narrow
highlight varied by several hundredths of a normalized axis; bounds checks do not
prove that a predicted center lies inside the mark. Simulator replies establish
routing and normalization, not visual accuracy or physical gesture recognition.
