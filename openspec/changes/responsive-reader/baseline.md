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

### Direct nearest capture-only measurement, sourcea9e1716
Reviewed source `a9e1716dfd2e2c199ff21f0cb98c93f1970f0316` passed the exact
pixel/alpha oracle, all7 screenshot tests and strict clippy. ARM capture helper
build passed1m30s, SHA256
`1e04e37ccbe504a94a5fd993731807438fee189e85d3608bc74a7b90d148cfdb`.
Three captures on the unchanged original notes page took
**462.843,489.112,443.368ms**, median462.843/max489.112 versus baseline
median752.569/max754.901ms. Median reduction289.726ms (about38.5%) is for full
capture only. Resize spans21.408,24.328,21.294ms versus319.406–321.007ms before.
All three output PNGs match the baseline SHA256 c1711658... byte for byte; the
new output was also visually inspected. Debug logging remains enabled in both
batches; observer/logging overhead is included, no correction or percentile claim.
Installed service remained active PID1445/restart0, no journal/input/UI changes.
This is not a measured full-workflow speedup, Paper Pro hardware proof, or REM9
acceptance. Double conversion/serialization and other waiting work remain.

### Readiness retest, source9330b4c
The separately reviewed readiness implementation passed165 host tests, strict
clippy, ARM release build3m05s and AArch64 build1m31s. Native Reader SHA256
`bff62ce6ff735cd4dbd234ced03e7da111453f2cd170842dfafd784b01da3c10`.
A bounded triggered attempt on the same ordinary tool/document observed initial
readiness in958.508ms and post-return readiness in3022.553ms. Both style leases
acquired successfully; no143,991 refusal or recovery journal remained. The source
failure marker was visually verified and then its exact owned paths were erased;
clear corner and actual Fineliner Black/Medium were checked afterward.

This was **successful invalid-successor handling, not successful Q&A**. The next
page classified Invalid, the Reader verified return to source and drew the proper
failure marker; no answer was typed and notes retained the same d3b4... hash. The
generic “Iteration completed successfully” log refers to handled control flow.
Post-run visual inspection of the successor shows a large serif Reader header,
whereas the preserved global cached header is smaller sans-serif; that mismatch
is a plausible classifier cause, not a captured in-operation classification-frame
diagnosis. Do not silently relabel it an intentionally occupied-page fixture or
count it as an answered workflow. Future successful tests need a valid configured
header/blank successor; broader header fallback remains separate roadmap scope.

Active inner time55.967973s, provider6.843142+3.365465s,38 captures. Acquisition
scopes12.395734+14.438697s; cleanup5.471845s and final restoration/verification
0.974589+0.983519s. These include nested readiness observations and are not a
performance improvement claim. Repeated ordinary/nondefault runs and visible
timings remain open. Original document/header/notes/binary restored again;
installed service active PID1445, restart count0, no test process or active journal.

Successful repeated ordinary/nondefault primary/secondary runs, slow/error/history
coverage, visible timing and observer overhead, current budget review, completion
sequencing, exact-pixel optimization, and all final delivery gates remain open.
No budget reduction or successful-Q&A waiver follows from these measurements.

### Read-only eligibility diagnosis, source6e1b2c4
The exact production classifier/decoder was exercised without a model call or
threshold change. ARM diagnostic SHA256
`1d041d1c6bf6b90aa28ac3de6cc2a5b737e956ca33fab956a8e4332268a9b422`.
Original notes native input SHA256
`c17116581603fd301152e833ee5a17d09506e8ae66af9c9efda169df4b9ec897`
classified ExistingQA; disposable notes input
`ab2fdd38ddcac229a3256de83a1891334c8aa79cd27304c87f24e047b7752ebc`
classified Invalid. Both used the unchanged saved reference
`6faae9628f71287149719e8afb2995a667628ce783b9a3e37ae8b48a1102a323`.
Each capture was bracketed by equal session, observed document/page/visit and
reference bytes. Session30974:100545844; original document46e07fc5-a3b5-4a0d-a71c-804a999fd2c7
visit1:75; disposable documente7f661f1-db6f-4dfc-854a-b38aff7f75de visit1:110;
both pagef39ae285-3e0c-43dd-b27c-866dff7a24cd. Both images were visually inspected.

An offline controlled replay of the exact disposable input with its own top150
rows as reference classified ExistingQA. Only the reference changed. This
establishes the saved-reference mismatch as sufficient to explain the current
rejection. It does not capture the earlier failed iteration's actual input or
reproduce workflow capture_clean/settling, and is not semantic header recognition.
The next successful integration fixture must have a correctly configured reference
or a blank successor; do not broaden eligibility in this performance change.
Original page restored visually; both notes hashes, header and installed runtime
unchanged. Service active PID1597, NRestarts0, no active recovery journal. Diagnostic
source passed strict all-target clippy and independent source review; no paid retry.

### Owned-image capture, source3062716
All168 host tests, strict all-target clippy, formatting, OpenSpec strict validation
and independent source review passed. ARM binary/examples build1m39s. Capture-only
helper SHA256 `f8fc3ba23b85a5a263f8688667671bd51d329d2a98c97f2abec0eb86f47c7475`.
Three interleaved encoded/pixel-only pairs on the unchanged original notes:

| Path | Individual capture.total milliseconds | Median | Maximum |
| --- | --- | --- | --- |
| Encoded native+overview | 303.503,302.977,280.277 | 302.977 | 303.503 |
| Owned normalized pixels | 185.921,183.131,186.394 | 185.921 | 186.394 |

The image-only diagnostic saves its artifact after capture.total; that save is
not included. Real status observation additionally reads/brackets ownership and
classifies pixels, so these are capture costs, not total status costs. All six
PNG hashes exactly matched c17116581603fd301152e833ee5a17d09506e8ae66af9c9efda169df4b9ec897;
a new image was retrieved and visually inspected. Debug timing enabled as before;
logging overhead remains included/unisolated. Service stayed active PID1597 with
zero restarts, no model call/input action. No Paper Pro hardware or full-workflow
performance claim. Five-observation toolbar convergence needs a real elapsed
deadline before full integration tests because faster capture shortens that old
implicit allowance. Other sequencing and acceptance gates remain open.
