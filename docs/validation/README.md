# Hardware fixtures

Use the repository's reader-buddy-testing skill before running these fixtures on
an authorized development tablet. JSON stroke fixtures and their Python generators
exercise known question, shorthand, ambiguous-ink, and missing-question cases with
`examples/hardware_probe.rs`. They are synthetic input, not a human handwriting study.
Inspect current UI state and adapt coordinates before replay; do not run blindly.

The reference document is Gundlach and Merkowitz,
[Measurement of Newton's Constant Using a Torsion Balance with Angular Acceleration Feedback](https://arxiv.org/pdf/gr-qc/0006043v2).
Its four-page PDF SHA-256 is
`332446943966f6c3111a09c2df05673a94282ea247bf78c946f2c6e6d479e1f6`.
Prepare a native blank notes page after the question page. Preserve original PDF
bytes/page order, clear only test ink between cases, and inspect the answer page
after every accepted or rejected question. A drawn X alone cannot prove no answer
was written; capture both state and sanitized logs.

PR #10 contains [accepted-question evidence](https://github.com/s116821/ReMarkableBuddies/pull/10#issuecomment-5687876142),
[rejection evidence](https://github.com/s116821/ReMarkableBuddies/pull/10#issuecomment-5687876336),
and [capture/build evidence](https://github.com/s116821/ReMarkableBuddies/pull/10#issuecomment-5687876514).
Only the seven images embedded there remain tracked as a fallback for unavailable
GitHub attachment uploading. Their comment URLs use an immutable commit. One-off
logs, superseded screenshots, and run reports belong in local evidence storage,
not the source tree; reusable fixtures and diagnostic probes remain here.

## Bounded return-navigation check

With the normal service stopped and exclusive authorized dev-tablet access,
`hardware_probe return-check` captures the current page, attempts one forward swipe,
then invokes the same single-attempt recovery used by Reader Buddy. It draws the
failure X and saves the resulting screenshot. This is an offline input/capture check;
it does not make a model call or test answer classification.

On a page with a successor, expect `Returned` and the X on the captured source.
At the document end, expect `AlreadySource`, no reverse swipe and the X on the last
page. Inspect before/after images. Failed swipe and capture cases are exercised with
scripted navigation doubles in unit tests; do not report those as physical failures.

## Activity indicator check

With exclusive authorized dev-tablet access and the normal service stopped,
`hardware_probe indicator-smoke` captures the clean page, draws Preparing,
AnswerPending and AnswerReady nested triangles plus the auxiliary circle, then
clears its owned paths. It writes `/tmp/reader-buddy-status-before.png`, one
`/tmp/reader-buddy-status-<stage>.png` per stage,
`/tmp/reader-buddy-status-active.png` and `/tmp/reader-buddy-probe.png` after cleanup.
Retrieve and inspect these images; compare the status region and neighboring ink.
The uninterrupted stroke cadence is 333 ms; diagnostic screenshots between stages
add delay and are not runtime scheduling evidence. Native tool acquisition and
restoration add separate overhead that must be measured, not hidden by cadence.

For a controlled investigation, `indicator-smoke-diagnostic` additionally enables
the existing debug dumps. On an unrecognized toolbar it preserves the actual
first rejected frame for that process/journal sequence as
`/tmp/reader-buddy-toolbar-refusal-<pid>-<sequence>.png`, with a private JSON
companion containing owner, last durable intent, expected controls and observation
times. It never recaptures or replaces the active baseline. Preserve these files
before another run; they contain page content. The ordinary helper and production
defaults are unchanged. A zero helper exit can still mean optional status was
suppressed after verified rollback: require visible glyphs, actual restoration
and cleanup evidence before counting a successful performance sample.

Exercise primary and secondary Highlighter with nondefault Fineliner color/width.
Capture actual menus before and after: persisted document preferences can be stale
and file equality does not prove current UI restoration. Status temporarily uses
black medium Fineliner, restores only potentially changed settings from captured
UI values, and verifies the original tool and slot. Open menus, unsupported layouts
and occupied clearance regions suppress drawing. A failed durable checkpoint or
unverified restoration stops further input and retains the private recovery journal
at `/var/cache/reader-buddy/status-style-recovery.json`. Do not blindly delete a
journal or restore preferences from stale document metadata; inspect its recorded
phase and actual controls before deliberate recovery.

`hardware_probe failure-code <code>` draws a persistent guarded X plus one distinct
segment. Supported codes are `selection`, `transcription`, `provider`,
`no-successor`, `invalid-successor` and `device`. This checks geometry and style
restoration, not six induced production failures. Explicit test-only
`erase-strokes <json>` may remove known owned diagnostic paths; visually verify a
clean corner before the next case. These offline probes do not validate model
recognition, answer placement, history availability or every native tool layout.

Temporary cleanup restores and verifies the original tools before eraser input.
The journal remains pending until cleanup, fresh session/page identity, original
closed controls and viewport checks pass. Native PDF erasure can redraw printed
glyph pixels beyond the status box. The post-erase viewport check permits that
empirical lower-right region only on pages with distributed landmarks elsewhere;
blank pages keep strict comparison. Sparse nonblank pages suppress status before
changing tools. This is not proof that all annotation ink survived the redraw
region: validate neighboring sentinel ink explicitly. Do not expand the region
or relax the strict pre-restoration comparison to make a failed test pass.

With explicit `READER_BUDDY_DEBUG_DUMP=1`, a refused page comparison writes fixed
`/tmp/reader-buddy-status-lease-before.png` and
`/tmp/reader-buddy-status-lease-rejected.png`. These are exact virtual frames for
the latest rejected observation, not guaranteed first-failure or native-resolution
captures. Preserve each pair and its journal/log before another diagnostic run.

REM-32 evidence in PR23 distinguishes e237 staged/native cleanup and live Q&A from
47b six-code geometry diagnostics. On the tested RM2, both Highlighter slots
preserved actual Red/Thick Fineliner settings and neighboring sentinel ink.
Acquisition plus cleanup took 44.170s (primary) and 49.753s (secondary), excluding
provider work: responsiveness remains REM-9/REM-35 work, not a claimed improvement.

History is conservative: the occupied-corner offline production-path control
passed repeated undo/redo with native text, styles, layout and opaque records
preserved. A corrected clean-page replay rendered the answer but refused arming;
immediate-before/later-applied records showed a load counter and line-to-tombstone
change, without capturing the internal settled comparison. Pre-REM32 also had
clean-append refusals, but identical cause or unchanged workflow timing is not
proven. Keep strict guards and report unavailable history; do not force a pass.

### REM34 CLI and native capture fixtures

`tests/fixtures/status-style/native-page-overlay.png` and
`native-page-overlay-settled.png` are RM2 firmware3.28.0.172 captures from the
6e685cc CLI correction validation. The first is preattempt, the second a later
settled source view. Their names describe visible bottom navigation chrome;
they are NOT the exact internal rejected-lease before/after pair. They also
contain PDF redraw differences (first strict comparison difference586,824).
The regression proves strict pre-mutation refusal on changed native frames,
not the isolated cause of live refusal143,991. Two bounded live attempts reached
correct independent question reading but wrote no answer; preserve that limitation
for REM9 availability work and REM35 integrated acceptance. No guard was relaxed.
The connected cursive input is a previously constructed synthetic native fixture,
not a new human-handwriting sample. `docs/validation/images/rem34` contains a
scripted simulator answer (approximate bitmap glyphs) and restored original tablet
view for public evidence; these are separate evidence categories.

`hardware_probe indicator-current-tool` is an explicit development-only candidate
path, not normal Reader behavior or a tool-admission claim. Use only reviewed
builds on verified disposable pages with a manually verified Black Fineliner.
Preserve its V3 journal and stop after any guard/cleanup failure; do not retry or
replace the baseline. This mode never opens a toolbar menu. It exercises the
production geometry/input methods with fresh per-stroke/per-eraser guards; actual
width/slot coverage, cleanup footprint, input observer continuity and full Q&A
acceptance remain required before switching the normal constructor.
