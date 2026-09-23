# Initial measured baseline — September 22 local / September 23 UTC

This is partial baseline evidence, not completed REM9 acceptance. Runtime source
`f38748da89fdbebfdedca17bc475e65b4305dd04` contains measurement only; no wait,
guard or pixel optimization. Independent measurement review found no blocking
defect and explicitly did not approve the full performance/Q&A gates.

RM2 firmware 3.28.0.172; ARM release build passed in 2m14s and AArch64 in 1m48s.
These durations are build times, not device latency. No Paper Pro hardware test.
Reader SHA256 `95aad3e0bd95fc4baded465f3bce33a29488178e21b763130c81d44b45ecadde`;
capture helper `df745d56a2ef1834ea644b8859646972210c5e0247a3e6e75e9e245f5fa919b8`.
The original installed runtime was preserved, not replaced by this test build.

## Capture-only baseline

Three independent helper processes on the unchanged original notes page, with
debug logging enabled, took **754.901, 751.572, 752.569ms** for full capture
(median752.569, max754.901). All three PNGs had identical SHA256
`c17116581603fd301152e833ee5a17d09506e8ae66af9c9efda169df4b9ec897`;
the image was retrieved and visually inspected. The installed service stayed
active and no input devices were initialized by this helper.

Nearest resize took319.406–321.007ms; each capture performed raw conversion and
native PNG encoding twice, around48ms and84–87ms per invocation respectively.
These are nested spans, not additional time to add to capture.total. Logging
overhead is included and has not yet been isolated. Different page image content
can change codec cost; this notes-page microbenchmark is not a full interaction.

## Triggered full-workflow attempt: failed availability

One bounded normal-loop run with explicit diagnostic images, synthetic qualified
hold, actual Primary Fineliner Black/Medium verified in the menu, disposable
technical-paper fixture with connected cursive “why flat plate?” and outlined
flat-plate paragraph. Source and selection were visually checked. The original
service was stopped for exclusive input. This is synthetic input on real hardware,
not a physical finger/human-handwriting test.

- Trigger-release observation to first status **input command**13.402272s and
  first provider request13.889031s. These are command proxies, not first visible
  feedback. Trigger dismissal was103.369ms; native first-visible timing remains open.
- Two provider request scopes15.013544s and4.002051s. Proposal and independent
  reading agreed on the actual question; relevant proposal remained unwritten.
- First acquisition12.314597s including initial observation; its inner existing
  acquisition log11.324802s excludes that observation. Second acquisition failed
  after4.067833s, including attempted verified rollback.
- Cleanup5.445451s includes actual tool restoration before erasure. The later
  `status.restore`972.744ms is final cleanup/style verification; it is **not** the
  whole tool-restoration cost. Total of these nonoverlapping top-level status
  scopes22.800625s includes failed reacquisition and is not successful readiness.
-27 full captures24.872475s aggregate, median929.528ms, max979.281ms.20 status
  observations19.199025s aggregate. These nest inside other phases; do not sum
  these totals with status/provider/wall timings.
- Active inner workflow52.843297s ended with the same guard refusal at143,991.
  No answer was typed; target notes SHA256 remained
  `d3b4c5395affda98b940c9322d17a5432449670ef4dec2623efe160c457df85e`.

One additional capture-only observer ran during the attempt; observer and logging
overhead are included/uncorrected, and this is one failed run, not a successful
baseline or repeated latency distribution. All recorded timing spans paired;
scope-end records do not turn failure into success.

## Exact internal refusal pair

The retained lease baseline and rejected frame differ by1414 pixels at threshold8,
all within x138–629/y991–1011. First difference is **143,991**; no differences above
row984. Visual inspection identifies the disappearing bottom page-navigation
overlay. This isolates the changed region for this actual refusal; it does not
establish which preceding operation caused the overlay or prove the cause of every
earlier REM34 failure.

Reusable exact fixtures:

- `tests/fixtures/status-style/native-footer-before.png`, SHA256
  `89978528a468ec45502901ae6da636774ae12bab9761146db12464599963b8af`.
- `tests/fixtures/status-style/native-footer-after.png`, SHA256
  `b68b30179bc327f25d54b5eaf363676e29d5883f15599f288a51386d75c7d086`.

The regression requires unchanged strict refusal before any toolbar input or
checkpoint. Do not mask out the footer in an active lease. Investigate waiting
for an observable usable state **before** committing a lease baseline, with fresh
owner/session checks, deadline/cancellation, unchanged safety and delayed/stale/
permanent-overlay tests. Never refresh an active baseline to forgive page changes.
Dump filenames retain the latest rejected observation in the attempt, so do not
claim this is necessarily the first rejected observation if rollback also refused.

Recovery was sequence0/Prepared with all mutation flags false. Actual original
Fineliner Black/Medium and clear corner were verified before preserving that
record outside the active recovery path. Original document, notes hash, header
hash, installed binary and service were restored; test process exited. Installed
service active PID1050, restart count0 at restoration. Raw logs/images remain in
the task-local evidence folder; preserve failed attempts alongside later results.

## Remaining gates

Successful repeated ordinary/nondefault primary/secondary runs, slow/error/history
coverage, visible timing and observer overhead, current budget review, completion
sequencing, exact-pixel optimization, and all final delivery gates remain open.
No budget reduction or successful-Q&A waiver follows from these measurements.
