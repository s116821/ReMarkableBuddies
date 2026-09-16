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
