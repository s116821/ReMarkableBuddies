# Local Reader simulator

Run the real Reader workflow on Windows, Linux or macOS without SSH, tablet input
devices or an API key:

```sh
cargo run -- --simulate docs/simulator/scenarios/blank-answer.json
cargo run -- --simulate docs/simulator/scenarios/failed-return.json
cargo test --test simulator
```

Each scenario writes `page-0.png`, `page-1.png`, etc. and `report.json` beneath its
configured output directory (bundled scenarios use `target/simulator/<name>`).
The report contains exact text, newly emitted X counts, unchanged-page results,
active page, virtual elapsed milliseconds, model-call counts and ordered actions.
Expected-result mismatches exit nonzero **after** saving results. Expected injected
failures can pass when the recorded behavior matches the assertions.

All model replies are **scripted and offline**. The application still builds its
real prompts and images, parses proposals, independently compares transcriptions,
classifies screenshots and executes its real recovery/rendering decisions. This
tests application behavior, not whether a model can read the handwriting. The
scripted engine validates that image content is decodable and traces content counts.
It does not connect to OpenAI even when credentials exist in the environment.

## Scenario format

Start from a maintained JSON file in [simulator/scenarios](simulator/scenarios).
Unknown fields and invalid states fail before execution. Page indices are zero
based; operation call numbers are one based and span the whole scenario.

| Field | Meaning |
|---|---|
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
| `expect` | Optional `active_page`, `model_calls`, exact `text` by page, `x_count` by page, `unchanged_pages`, exact operation counts and ordered error substrings. Errors default to none. |

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
display latency. The simulator does not emulate evdev slots/raw device scaling,
kernel input injection, xochitl typography/cursor state, native file serialization,
paper orientation, reboot, framebuffer allocations or human handwriting quality.
It cannot prove hardware compatibility or catch every native output/layout defect.
The real adapter retains existing Screenshot/Touch/Pen/Keyboard operations and
normal Linux cache behavior. Targeted hardware checks remain required for those
areas, especially after changing the shared boundary.

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
