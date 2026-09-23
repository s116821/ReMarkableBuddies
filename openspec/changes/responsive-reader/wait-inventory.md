# Wait inventory and measurement contract

Source inspection on September 22, 2026. This is the baseline inventory, not a
claim that the proposed replacements or their native validation are complete.
No timing or input behavior has changed in the measurement checkpoint.

## Supported observations found in this repository

- `input_observer.rs` reads Linux input frames and checks device identity; these
  are input activity/ownership signals, not xochitl rendering acknowledgements.
- `native_page.rs` reads document/page/visit and process identity. Native history
  additionally parses persisted text and requires exact expected content. Files
  can lag visible state; a change notification alone would not establish success.
- `status_style.rs` classifies fresh toolbar pixels and verifies page identity and
  unchanged content before input. Persisted tool preferences are not UI authority.
- `openai.rs` receives the result of the one active scoped request through a
  capacity-one channel. Its worker is joined even if the progress callback fails.
  Current cancellation therefore does not promptly abort the HTTP request.
- No xochitl UI-completion subscription is implemented in the current dependency
  set or device adapters. This does not establish that no external API exists.
  Native interface investigation remains open; do not invent an event source.

Read-only RM2 inspection (firmware 3.28.0.172, xochitl PID 30974) found two unique
system D-Bus connections for xochitl and no well-known xochitl UI service name.
Bounded five-second introspection of each root returned `Access denied`. No
objects/signals were discovered. Do not change bus policy or bypass that denial.
Thus no usable supported UI completion subscription has been established on this
tablet; fresh pixels plus existing owner/session observations are the available
fallback. This is evidence of this inspection's limit, not proof of global API
absence. No tablet configuration or service state was changed by inspection.

## Current waits and next verification

| Caller / baseline wait | Actual required completion | Decision / remaining evidence |
| --- | --- | --- |
| `main.rs`, 1000ms after device construction | Virtual input devices usable by the application | `Keyboard::owned_sysfs` establishes registration only. Investigate input readiness; keep this one-time wait unchanged until consumer readiness or a justified exception is established. |
| `xochitl_integration.rs`, 500ms after swipe; orchestrator/navigation, another 800ms | Correct destination, same document/session, fresh rendered page | Replace stacked assumptions with bounded owner/page and fresh-content verification. Issue exactly one swipe; a missing signal must not repeat navigation. Current image similarity alone is not full destination identity. |
| `Workflow::is_valid_answer_page`, 500ms | Settled, eligible answer page | Fresh capture plus blank/header predicate is available; correlate to requested destination. Repeated identical captures alone cannot establish that navigation occurred. |
| Orchestrator header save, 500ms | Complete header rendered before taking reference | Native exact persisted header is available on the verified RM2 contract but lags rendering. Require fresh visible header verification too; unsupported hardware needs an explicit safe fallback. |
| `RealDevice::press`, 250ms contact + 100ms after release | Recognized tap, then requested toolbar state | Contact duration is a physical-input candidate exception, not a measured minimum. Post-release delay should become bounded fresh UI polling. Preserve release-on-error and journal-before-input. |
| `Lease::transition`, up to five captures without explicit pacing/deadline | Exact slot/menu/grid/color/width predicate under unchanged owner/content | Use monotonic deadline and paced fresh observations with cancellation. Keep pre-mutation and every post-mutation guard; don't retry the press when observations lag. Observation cost itself must be bounded/measured. |
| `RealDevice::status_clear`, 100ms after erasure | Owned marks gone, surrounding content and original style preserved | Use bounded fresh cleanup evidence and existing final lease proof. Empty corner alone is not full restoration. |
| `NativeHistory::settled`, 100ms polling, 30s deadline, three equal readings | Complete expected parsed content under owner/session/input guards | Existing bounded polling, not a fixed completion assumption. Check deadline after expensive observations too; preserve external-input cancellation and exact content. Persisted latency and visible latency are separate. |
| `NativeHistory::wait`, 10ms polling | Input reducer emits qualified action or invalidation | Legitimate polling cadence; investigate descriptor readiness only if measured relevant. Preserve contact state across invalidation and timeouts. |
| `OpenAI::execute_with_progress`, channel receive timeout at indicator cadence | Result for active request or failure | Existing real completion event. Serialized progress callback can postpone consumption; measure HTTP span separately from callback/join wait. A failed callback must never allow late provider success to resume device input. |
| Indicator deadline, 333ms stages/auxiliary | Feedback animation cadence | Intentional cadence, not readiness proof. Keep visible-feedback measurement separate from menu work and input-write timestamps. |
| Swipe 50ms initial contact + 15 x 10ms steps | Recognizable continuous gesture | Physical gesture candidate exception; baseline retained, no claim these are minimums. |
| Keyboard sync/key stages 1/10ms, extra caret composition 50ms | Complete characters including native dead-key behavior | Input pacing with known caret correctness evidence from prior native work. Multiple waits mean roughly 40ms per ordinary character, not the historical inventory's 10ms. No blind reduction. Verify exact persisted and visible output with fixed text length. |
| History keyboard 50ms pacing / 60s deadline | Owned, bounded edit command | Keep ownership checks and native complete-content verification; do not replace with status activity. |
| Pen/touch 1/5/10ms event/row/path waits; trigger tap 100ms | Recognized strokes/contact sequences | Physical-input candidates, not measured hardware minimums. Keep until separately validated. |
| Reader hold 2s and history contact qualification | Intentional gesture qualification | Preserve; report separately from trigger-release-to-feedback. |

The historical 200ms body-mode waits are absent in the current orchestrator.
Do not report savings against removed code. Each remaining fixed-wait candidate
needs native validation before being called a necessary exception. No Paper Pro
native timing evidence is available.

## Measurement events

### First capture cost change
The nearest resize now copies the exact selected eight-bit channels directly,
using the same f32 center/floor/clamp sampling as image0.25.10's zero-support
Nearest kernel. Pixel-for-pixel comparison against the library covers normalized
native RM2 dimensions (modern and rotated legacy), Paper Pro dimensions, gray
levels, RGBA/alpha, upscaling and equal dimensions; invalid formats/empty images
refuse. Raw conversion/orientation, both PNG encodes, allocation discovery and
owner guards remain unchanged in this incremental step. No native speedup is
claimed from source checks alone; baseline.md records the subsequent three native
captures and their limits. The next owned-image implementation removes the second
native conversion/encode/decode from workflow capture and all PNG round trips
from status observation. Conversion and native serialization now have separate
spans. The old codec path remains a test-only oracle; full native integration and
repeated performance acceptance remain open.

Debug logs emit fixed `timing` fields: process ID, run ID, operation ID, phase,
begin/end, monotonic microseconds and elapsed microseconds. An iteration gets a
new run; the existing provider worker explicitly inherits it. Run zero denotes
operations outside an iteration (for example screenshot diagnostics). IDs are
local to one process; retain the source revision, binary hash and launch log with
every run. No prompt, key, endpoint, image, document ID or answer text is added by
these timing events. Existing non-timing logs may contain document/model content;
inspect them before publication. Logging remains controlled by existing levels.

Events stream to the configured logger without an in-memory history or new
background process. Each instrumented call emits two records. An end event means
the scope ended, including error or panic; it is NOT a success assertion. Pair by
process/run/operation and preserve incomplete spans if a process is killed.
Counts are counts of begin records, not begin plus end. Nested and overlapping
spans must not be summed as independent latency. Provider request time overlaps
status callbacks, and `provider.wait_with_progress` includes join/callback cost.

`capture.native_png` currently includes raw conversion and serialization, twice
per full capture, with `capture.raw_conversion` and `capture.native_serialize`
subspans; Paper Pro already supplies RGBA bytes so has no raw conversion span.
`capture.overview` includes the second native PNG, decode,
resize and overview encode subspans. `status.observe`
includes owner bracketing, screenshot, decode, metadata and classification input;
`status.acquire/cleanup/restore` include their nested observations. Sum all actual
leases per iteration but don't double-count their inner spans.

`reader.active` starts after trigger dispatch/iteration setup (or before immediate capture)
and ends when the inner workflow returns; `reader.iteration` also includes outer
cleanup and may include trigger waiting.
`reader.trigger_dismiss` begins immediately after observing the qualified trigger,
before the dismissing tap; it is a release-observation proxy with input-poll lag,
not a hardware release timestamp. The normal loop establishes its run ID before
idle dispatch so this span and the subsequent iteration can be correlated.
These are host monotonic timestamps, not simulated elapsed time or a claim of
first/last visible pixels. Rendering
logs include character count only. Bounded visible observation, logging overhead,
successful/error labels and repeated native baseline remain required.

Exact internal rejected-frame dumping already exists behind the resolved
`--debug-dump` setting in the normal executable; the immediate `reader_once`
example intentionally disables it. For refusal investigation use the normal
bounded loop with opt-in dumping and preserve both images after each attempt.
Do not substitute external pre-attempt screenshots for the internal pair.
