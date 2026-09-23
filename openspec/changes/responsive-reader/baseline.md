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

### Corrected-fixture baseline remains unsuccessful
One bounded old9330b4c attempt used a manually configured reference from the
visually verified disposable Reader Q&A header. This was controlled test setup,
not an automatic cache update or general eligibility change. Both a fresh native
diagnostic and the actual workflow classified the page ExistingQA. The run then
refused before rendering with `Page or toolbar changed during status readiness`.
Active scope38.718684s; provider7.750037+2.901032s; rejected readiness1.691933s.
No new answer, status mutation on the successor, or recovery journal; disposable
notes retained d3b4... hash. The old readiness function did not dump its compared
frames, so the later external screenshot does not establish the changing region
or cause. No further paid baseline retry follows without new evidence.

Original page visually restored, original/disposable notes/header/runtime hashes
unchanged; installed service active PID1987, NRestarts0. Source48b6d38 separately
passed171 host tests and both release builds (ARM3m12s, AArch641m43s), but has no
integrated native run yet. The subsequent diagnostic adds bounded opt-in exact
readiness rejection images/identities and a no-model production-readiness probe.
The shared cancellation latch/real-journal regression proves observer-open
failure or cancellation blocks subsequent acquisition rollback/cleanup input,
retains the unresolved journal and cannot be revived by a later good poll.

### Exact no-model readiness refusal, sourcee788aa0
ARM probe build1m33s, SHA256
`498846e0e46c571fbad153a5f4ef47af50e557b159babe939dd4a5d282fea175`.
After one diagnostic source-to-notes swipe, production classification ExistingQA
was followed by the same readiness refusal at1.968744s. No model, answer/status
ink or lease journal. The exact first-pending/rejected images are retained as
`native-scrollbar-before.png` (SHA256
`665a8cff369a9ef083c3a3bb5c2133c2673b51776fa2b2069e1ca92cfc45db9b`)
and `native-scrollbar-after.png` (SHA256
`ab2fdd38ddcac229a3256de83a1891334c8aa79cd27304c87f24e047b7752ebc`).
Both observations had identical documente7f661f1-db6f-4dfc-854a-b38aff7f75de,
pagef39ae285-3e0c-43dd-b27c-866dff7a24cd, visit1:114, session30974:100545844.
Reference af13e3f521d25efc77b2b95fe8dd51890d61aede804e0ef18c36f994fd62f115
was unchanged before/after the exclusive-writer probe.

Visual inspection shows the right scrollbar and bottom page label disappearing.
At the production difference tolerance8,3480pixels differ,3237 above row984;
all those3237 are within x735..739, first(736,137). The remaining243 are in the
footer. All other pixels are unchanged. Preserve this exact pair as a strict
readiness refusal regression. This identifies the cause in the no-model probe,
not an uncaptured earlier iteration, and does not permit a content mask or active
baseline refresh. Navigation/capture sequencing must establish completed page UI
before classification/lease preparation. Its diagnostic visual difference plus
same document/session is not proof of an exact requested successor in general.
Original document/header/notes/runtime restored; service activePID2145,NRestarts0.

### Verified navigation probes and first rendered Q&A, adfe055
Exact source `adfe055530b979a8512de917ab55a092d30886b3` passed ARM and
AArch64 builds. Full180 host tests passed at3f2d020; subsequent diagnostic
identity correlation and measurement-only changes passed strict all-target clippy.
No-model probe SHA256
`b0fab28c1f274d91864ff5e545eea7728028d54b89a1550227a166bf0cf29326`.
One Next and one Previous each returned Settled, expected UUID and exact
document/page/visit/session through classifier and readiness. Visually inspected
notes/source ready images had no transient scrollbar/footer. Next ExistingQA,
Previous Invalid as an answer page (expected for the source PDF). Navigation
5.561460/5.259777s includes source observation/swipe before the postgesture
deadline; readiness177.135/178.844ms. Diagnostic800+500ms waits remained.
These are individual readiness samples, not full-workflow or latency acceptance.

Normal Reader binary SHA256
`b8ccf1382486ef3d144315432f70f4e9ad3b0afbecbfb213c56b4db6c0ca9374`
then ran once on the same legitimate manually referenced fixture. Actual ordinary
primary Fine/Black/Medium was visible during acquisition. Both leases completed;
new answer and both position delimiters were visually inspected. Recorded costs:

| Phase | Source lease (ms) | Answer-page lease (ms) |
| --- | ---: | ---: |
| Acquisition including readiness |3313.345|3380.167|
| Original tools restored within cleanup |575.617|566.527|
| Erasure and its immediate verification |1720.070|531.405|
| Whole cleanup (includes preceding two rows) |2296.225|1098.406|
| Final restoration/cleanup verification |255.853|204.161|

The union of acquisition/whole-cleanup/final-restoration intervals is10.548118s;
do not add nested rows again. This excludes animation, scheduling gaps and provider
time, and is not first-visible-feedback evidence. Do not compare it as like-for-like
to the older44/49s nondefault-slot runs. Provider requests18.929243/3.494736s;
active workflow90.394073s. One external image-only observation during rendering
adds observer cost; this run does not establish exact first/last visible timestamps.

**Unresolved:** post-answer native history persistence waited30.005685s then
reported expected complete text absent; history was disabled. The log's successful
iteration means rendered output, not full history/performance acceptance. An
offline parse of exact prior notesd3b4 and resulting notes0f02b3 shows the823-character
old text retained as prefix and nine extra newlines before the395-character new
block. The actual in-process pre-render snapshot was not dumped, so attribution
to cursor/materialized blank paragraphs remains a hypothesis; do not normalize
away the discrepancy or relax history ownership checks.

Original notesc284, header6faae and installed runtimecaa5 hashes were preserved;
original document visually restored, service activePID2834,NRestarts0,no journal.
Disposable answer changed as authorized, and source native bytes also changed
after indicator drawing/erasure; byte equality is not claimed for either.
Repeated comparable success/error/nondefault-slot tests, visible timing evidence,
neighbor ink/UI verification and remaining dominant stalls are still open.

### No-model append control, 8443818
The explicit append probe (SHA256
`dbf9aa614951dbb893895862965ce54d0e3bd19f6ed0f5a511a3cb5fa3861982`,
ARM build1m22) ran once after reopening the disposable answer page. Selected,
before-body, immediate-after-body, before-render and immediate-after-render native
snapshots all had1227 characters/33 paragraphs. Final persisted content had1365
characters/38 paragraphs and exactly equaled the observed before-render content
plus the138-character requested block. All identities matched visit1:118 in the
same document/page/session; the visible block and both delimiters were inspected.
Persistence waited10.003155s. This control has no navigation or status ink, and did
not reproduce the nine extra newlines of the prior normal iteration.

Exact text readiness is **not history arming**. The recorded native root layout
was unchanged, but scene-record comparison differed in PageInfo's first counter
(3 to4; the local rmscene0.8 parser names it loads_count). Production History.arm
therefore remains conservative. No field was ignored or normalized. Its semantic
significance requires controlled evidence before changing ownership comparison.
The small right-side dash visible after scrolling also exists in the prior native
fixture as a two-point stroke; its changed coordinate representation is not proof
of either damaged or preserved ink. Full semantic/visual preservation remains a
gate. Original document/header/runtime were restored and visually/hash verified,
service activePID2976,NRestarts0,no journal. No live model call in this control.

### Repeated ordinary indicator cycles and nondefault failure

Three matched single-lease cycles per build used identical three-stage triangles
and six auxiliary ticks on the disposable source page. Baseline adfe055 and
candidate e4b263a each completed all three ordinary Fine/Black/Medium cycles,
cleared the reserved corner and left no active journal. Actual primary settings
were visually verified. These are indicator-only cycles, without provider,
navigation, answer rendering or history; they do not complete full-workflow gates.

All numbers below are milliseconds. Tool restoration and erase/verify are nested
inside whole cleanup. Total is the union of acquisition, whole cleanup and final
verification intervals; never add the nested columns again.

| Build/run | Acquire | Restore tools | Erase/verify | Whole cleanup | Final verify | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| adfe/1 |3381.317|573.519|1732.152|2306.282|229.690|5917.270|
| adfe/2 |3309.283|562.441|1787.239|2350.228|206.682|5866.173|
| adfe/3 |3316.605|563.864|1757.986|2322.425|207.231|5846.241|
| e4b/1 |2341.197|567.588|1749.709|2317.810|210.949|4869.936|
| e4b/2 |2373.336|556.221|1698.113|2254.894|198.068|4826.278|
| e4b/3 |2410.941|567.064|1851.326|2418.902|247.115|5076.938|

Median total 5866.173 to 4869.936; maximum 5917.270 to 5076.938.
Acquisition median 3316.605 to 2373.336. Cleanup is largely unchanged.
Before/final paper comparisons differ by 35-222 pixels in the lower-right redraw
region, so zero page-wide change is not claimed. Offline parsing found exactly
equal properties for all 122 recognized source Lines, including after the failed
primary cycle below; parser warnings about unknown bytes prevent full ink proof.

The first nondefault primary baseline (Highlighter/Yellow/Snap-to-text on, hidden
Fine/Red/Thick) stopped on an allocation-header read EIO during post-erasure
capture. Acquisition excluding readiness was 5.712405s, tool restoration 6.508239s,
and whole cleanup 8.124949s. Ten owned paths had been erased, but capture failed
before raw-frame reading, leaving PendingCleanup sequence 11. No final verification
or successful total is reported. No candidate primary or secondary cycle followed.
The old error lacks candidate address/mapping evidence; its cause is unresolved.

Deliberate recovery first verified no running writer, the same document/page/
visit 1:119/session 30974:100545844, a freshly captured clear corner and actual
recorded Highlighter/Yellow and hidden Fine/Red/Thick settings. The journal was
preserved (SHA256 60a317ddba333402cbbe9535d0b2beb61d7b4c90b5ce3f9e2213ab4fac32ffff).
Pre-batch Fine/Black/Medium, original document, header and installed runtime were
then restored and visually/hash verified; service active PID3613, NRestarts0,
no active journal. Secondary was not changed in this batch.

Diagnostic-only capture context now records candidate address, sampled mapping
range/permissions, PID and failed syscall phase. A failure-only second maps
snapshot reports whether mappings changed and whether the address remains mapped.
It does not retry memory reads, cache an address or infer success. The regression
retains the underlying OS error and asserts one read only. Native EIO reproduction
and diagnosis, repeated nondefault/error cases, all visible milestones and full
history/performance acceptance remain open.

### Bounded EIO controls, 4430db8 and original baseline with diagnostics

Capture-only ARM build 4430db8 passed (1m31); binary SHA256
c1cc74398f3600407229da3bc35fe9920714e70e66102608381edfcc315ff28c.
Thirty idle captures and twelve captures during each of one menu opening and
closing all passed. All idle images and the final closed-menu image had SHA256
c17116581603fd301152e833ee5a17d09506e8ae66af9c9efda169df4b9ec897.
Actual Fine/Black/Medium and original document/page/visit/session were verified.
These negative controls do not identify or fix the EIO.

One original nondefault-primary workload then used pinned diagnostic source
492c626a1d08d6065a9486c801672af492ea1aa8: parent adfe055, with ONLY the reviewed
4430db8 screenshot context/error-chain regression applied. No later optimization.
ARM build1m28, hardware probe SHA256
6a81067222206686b2ef36525f0d5e114eb7c1967f211e057ca998ee82bdc025.
This diagnostic completed once, without EIO. All twelve journal records (0-11)
were retained through a read-only file descriptor, including after normal unlink.
No active recovery record remained. Exact owner was e7/a718/visit1:119,
session30974:100545844 before/after; full maps differed across the cycle but this
is not evidence about the earlier failed syscall. All122 recognized native Line
properties still match; unknown-format warnings retain the preservation limit.
Highlighter/Yellow/Snap-to-text on and hidden Fine/Red/Thick were visually restored,
and the reserved corner was clear. No additional cycle followed this diagnostic.

Acquisition6109.121ms; tool restoration6598.072ms; erase/verify1648.383ms;
whole cleanup8247.032ms; final verification233.462ms; parent union14589.594ms.
There were43 status observations (median182.702ms),50 total captures and10 presses.
These single diagnostic values, including observer overhead, are not repeated
performance acceptance. They directly show that nondefault tool restoration
remains a major cost. Repeated acceptance sampling must keep failed samples and
stop/recover on first failure, with diagnostic context on both comparator builds.
Do not introduce speculative retry or call this intermittent error fixed.

Original Fine/Black/Medium, document, notes c284, header6faae and runtimecaa5 were
restored and visually/hash verified; service activePID4894,NRestarts0,no journal.

### Fused image-only conversion, 87aab2d

Exact 87aab2dd3eec52a2e379230af759fc82bf0569c8 passed183 host tests, strict
clippy, OpenSpec validation, ARM build2m53 and AArch64 build1m40. Independent
source review found no blocker. Three alternating read-only pairs compared
4430db8 with the fused image-only screenshot binary (SHA256
79c89bfdda432f0df63a6ef0be0eef858859b73046210f129805cbd7b079a27f).
All six normalized PNGs exactly matched c1711658 (full hash above); the candidate
image was visually inspected. Owner46e/f39/visit1:75/session30974:100545844 matched
before/after. The installed service and tools remained unchanged.

Capture totals in milliseconds: old172.501/171.401/164.759 (median171.401),
fused132.999/142.443/143.266 (median142.443). Fused conversion itself took
35.563/35.384/39.376ms, replacing old conversion plus resize. This is only a
three-pair idle microbenchmark, not marker or workflow acceptance. A subsequent
equivalent row loop removes the image iterator's per-pixel coordinate overhead;
its native improvement must be measured separately before claiming further gains.

The row-loop revision d3fbd5e passed the nine pixel regressions and strict clippy;
ARM build2m04 and AArch64 build1m46 passed, with independent source review.
Three new alternating read-only pairs remained exactly c1711658 pixels:
4430 totals158.471/185.472/162.587ms; d3 totals134.676/121.511/118.181ms.
Fused conversion27.370/24.708/24.724ms. No input or installed-runtime change.

### Matched indicator matrix stopped on nondefault toolbar refusal

Comparator492c626 is the original adfe baseline with diagnostic context only;
candidate d3fbd5e includes consolidation and fused capture. Each ordinary case
used the identical full three-stage plus auxiliary-tick cycle. Positive glyph
presence, cleared reserved corner, actual Fine/Black/Medium, owner and absent
recovery record were verified. All six final frames were visually inspected.

| Build/run | Acquire | Restore tools | Erase/verify | Whole cleanup | Final verify | Total union (ms) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
|492/ordinary1|3458.369|582.677|1765.428|2348.679|220.195|6027.223|
|492/ordinary2|3377.142|571.809|1678.535|2250.962|222.993|5851.077|
|492/ordinary3|3354.811|564.912|1767.246|2332.714|214.023|5901.529|
|d3/ordinary1|2088.209|430.519|1670.221|2101.230|162.923|4352.340|
|d3/ordinary2|2127.336|428.841|1692.508|2121.911|189.515|4438.741|
|d3/ordinary3|2104.778|440.717|1697.943|2139.151|171.172|4415.083|
|492/primary1|6126.568|6634.658|1737.310|8372.463|269.675|14768.687|

Ordinary total median5901.529 to4415.083ms, maximum6027.223 to4438.741ms;
observations16 to11. The nested columns are not added again. Both builds retain
the same observer/journal-preservation procedure; these are still single-lease
indicator tests, not complete Q&A or visible-milestone acceptance.

**Failed candidate primary1:** after SelectFine, acquisition refused with
“Status toolbar layout changed”, then verified rollback and suppressed every
indicator mark. The helper returned0, but the test's required positive acquisition/
erasure/restoration checks correctly failed and stopped the batch. No successful
timing is credited. Journal sequences0..4 show Prepared, OpenPrimary, SelectFine,
RestorePrimary, RestoreClose; original grid2 and no captured Fine style. No EIO
or active journal. The actual internal rejected toolbar frame was not saved.
Faster capture seeing a transient popover is only a hypothesis. Remaining primary
pairs and all secondary/error cases were not run.

Fresh owner remained e7/a718/visit1:119/session30974:100545844. Actual
Highlighter/Yellow/Snap-to-text and hidden Fine/Red/Thick were visually verified.
All122 recognized source Lines remain equal, with the same unknown-format limit.
Original Fine/Black/Medium and document46e/f39 were then visually restored; notes
c284, header6faae and runtimecaa5 hashes matched; serviceactivePID7789,NRestarts0,
no active recovery record or writer. No blanket unknown-layout retry is justified.

Current-tool calibration: e4b263a hardware helper (SHA416f6a3d910b13bb1e1fba55e6a6694159266c71ec7d3e2f2753438c471afeea)
drew one segmented triangle/X pattern using the manually selected primary Black
Thick Fineliner at pressure2630, then erased only recorded paths with physical
rubber input. No toolbar selection occurred during drawing/erasure. Native owner
reports match before/active/after;551 new dark pixels occupy x705..737,y941..974,
inside the reserved region; final clearance is white and toolbar pixels match
exactly before/active/final. Actual tool menu before/after verifies unchanged Thick.
All122 recognized preexisting Line properties compare equal after cleanup; unknown
native records remain outside parser proof. This single pattern does NOT establish
all widths/colors/slots, maximum eraser envelope, continuous auxiliary paths or
production lease correctness. Local artifacts: current-tool-thick* in outputs/rem9.
Original Fine/Black/Medium, original document and notes/header/runtime hashes were
restored and verified; installed service activePID8419,NRestarts0, no active journal.

Second current-tool calibration batch (same e4b263a helper/firmware/owner): primary
Black/Thin segmented pattern192 darkpixels bounds706..736/942..973; secondary
Black/Thick551pixels bounds705..737/941..974. Both clear after same-path physical
rubber erasure, actual pen tiles exact before/active/final, owner reports equal.
Actual menus verify original sizes after erasure. Across both batches all122
recognized preexisting Lines match the initial source; unknown-record limit remains.
Primary White/Medium closed tile differs160pixels>8 from Black reference and is
rejected without drawing. Secondary native Fine tile differs131pixels>8 from the
modeled shifted primary reference; preserve this false-refusal regression with a
separate exact secondary reference, not a looser threshold. Thin primary tile is
exact to original Black/Medium reference. OriginalprimaryFineBlackMedium and
secondaryBallpointBlackMedium (also temporarysecondaryFineBlackMedium) restored
visually; original46e/f39/header/runtime hashes match, serviceactivePID8743,
NRestarts0, no active journal/writer. Full188 hosttests pass on8756945. These are
segmented calibration cases, not V3 production lease/full Q&A acceptance; continuous
auxiliary paths, maximum eraser envelope, notes chrome and remaining widths/slots
still require verification before admission.

Native V3 diagnostic ddbd8dd (2026-09-23T05:21:57..05:22:15Z): one
primary Black/Thick Fineliner cycle with three recorded neighboring strokes.
Acquisition322.049177ms; cleanup parent6239.007ms (includes478.456ms preparation
and5760.006ms erase/verify); final status.restore failed after101.416ms. Do not
count this as successful indicator latency. Positive stage images and active
image exist; all10 owned paths were erased, but final verification failed.
The exact error chain reports allocation discovery candidate0x64ba1000 in sampled
anonymous rw-p region0x64ba1000-0x65c2a000, eight-byte header read EIO5,
maps_changed=true/current_region=unmapped. Preceding successful captures selected
framebuffer0x6d2f9008, so the failed discovery candidate is NOT established as the
framebuffer. Before/after batch maps both contain65297000-65c2a000; they are not
the exact failure-time snapshots. PID30974/start100545844 and page owner remain
unchanged in batch brackets. Later maps are evidence of change, not syscall-time
proof or proof that historical EIO failures share this cause.

No retry occurred. V3 sequences0/1/2 retain PendingCleanup; preserved journal SHA
0bd27c769a2370cb25b7e96e7a03c145fd278eea82c45f6285d95a16e1ef6fbd.
Separate later read-only inspection proves matching owner and clear corner, not
success of the original failed verification. Native125 recognized Line properties
(122 original plus3 neighbors) compare equal before/after the cycle. Deliberate
recovery removed only the three recorded neighbors;122 original recognized Lines
remain equal, unknown records are unverified. Actual menus before/after prove
unchanged Black/Thick Fine. Original Black/Medium Fine and original46e/f39 document
restored; notes/header/runtime hashes unchanged, serviceactivePID9361/NRestarts0,
no active recovery journal. Preserved original journal retired only after manual
verification. Local private artifacts: outputs/rem9/current-ddbd-primary-thick,
current-ddbd-sentinels, current-ddbd-recovery-observe and current-ddbd-*.png.
Pixel analysis of the failed ddbd8dd cycle: Preparing331, AnswerPending515,
AnswerReady602 and active604 new dark pixels, all bounded x705..737/y941..973.
Both pen tiles are exact at every captured stage. Later recovery screenshot has
clear statusROI and exact three neighboring pixel rectangles as well as125 equal
recognized Lines. This supplements, but does not replace, final verification.

Current-tool cycle4280f24: exact ARM helper SHA97c77954003d001e84af7e4d77929c1c9d6f48cecea2d03a6fefd7558e02748e,
ARM2m48/AArch1m58 builds and195 full host tests pass. One primary Black/Thick
cycle succeeds with V3 sequences0/1/2, no toolbar press spans, exact owner,
positive331/515/602/604pixel stages within705..737/941..973, unchanged tool tiles,
clear finalROI and three unchanged neighboring rectangles. All125 recognized Lines
including sentinels match after the cycle; unknown-record limitation remains.
No EIO occurred and no fresh-reacquisition log appears: native recovery-branch
execution is NOT proven by this success. Acquisition360.976501ms, cleanup6615.661ms
(includes530.223ms preparation and6084.876ms erase/verify), final399.910ms;
non-overlapping union7376.548ms. This single diagnostic is not an end-to-end or
ordinary6s budget pass. Deliberate removal of the three recorded sentinels leaves
all122 original recognized Lines identical. Actual before/after menus show Black/
Thick Fine; restore Black/Medium Fine, original document/files/service verified,
PID10017/NRestarts0 and no active journal. Both candidate slots, all admitted widths,
notes chrome, error indicators, suppression/core Q&A, history and full workflow
latency remain admission gates. Production constructor remains on the old path.

Secondary Black/Thick cycle2cde400 succeeds (ARM2m02;32 status-style tests,
strictclippy/fmt/spec pass). Helper SHA2f1135438e92123df29badf71e365eed8833184a841fba8fcf742ee9c8bf5da6.
Acquisition364.789859ms, cleanup6537.838ms, final390.282ms: union7292.909859ms.
Cleanup preparation531.454ms; erase/verify6005.906ms. Ten disjoint per-path guard
spans4282.665ms, physical injection1376.811ms, rearm36.182ms. Nested13 observations
2470.541ms and checkpoint4.431ms are NOT additive to those parents. No EIO or
reacquisition branch. Same331/515/602/604pixel progression/bounds as primaryThick,
unchanged pen tiles, clear finalROI, exact neighboring pixel rectangles and125
recognized Lines. All journal phases0/1/2 and no toolbar press spans verified.

Separate developer sentinel cleanup exposed persistence lag/discrepancy: the first
and later saved files retained the exact two-point top sentinel (123 recognized
Lines), despite visually clear captures. All122 original Lines were unchanged.
After fresh owner/native ownership proof, one deliberate same-path erase was made;
the immediate file still contained123 Lines. No further erasure. After closing the
disposable document, source-after-close.rm contains exactly122 original Lines.
Retain all snapshots; this does not establish a general flush mechanism or justify
forcing a product document close. It is separate from successful indicator cleanup
and reinforces that visible clearance does not establish native persistence.
PrimaryFineBlackMedium, secondaryBallpointBlackMedium and temporarysecondaryFine
Medium visually restored; original46e/f39 notes/header/runtime hashes unchanged,
serviceactivePID10743/NRestarts0/no active journal. Further native acceptance and
full Q&A/history/performance gates remain open.

Primary Black/Thin cycleb43b083 succeeds (ARM1m43, helperSHA2f01d03c8fc6e19740602c9a867426b8819e2b67489493884169fd8c35fd058f).
Stages131/197/240/249 new darkpixels within706..736/942..972, unchanged tool tiles,
clear finalROI, exact neighboring rectangles and125 recognized Lines. Actual menus
verify unchanged Thin/Black Fine. Acquisition370.746564ms, cleanup6524.580ms,
final404.321ms; union7299.647564ms. No EIO/reacquisition. Cleanup: ten pathguards
4287.504ms, physicalinputs1382.386ms, rearms36.208ms. The ten cleanup pixel-verification
scopes total117.537ms; they also include identity/control predicates, not pure pixel
CPU. All108 input-guard scopes in cleanup total148.455ms;39 recovery owner-guard
scopes270.045ms;13 status observations2464.059ms. These scopes overlap and must
not be summed across levels. Per-path interval analysis finds1756.056ms total
between each last input-guard end and its enclosing guard end (146.721..217.917ms).
That source interval contains lease bookkeeping, cancellation check and observer
drop. This supports investigating descriptor teardown; it does not independently
measure a kernel close syscall or establish its cause. Do not optimize pixel
predicates or join paths based on the earlier unattributed remainder hypothesis.
After single recorded-sentinel removal and document close,122 original recognized
Lines match exactly. OriginalFineBlackMedium/secondaryunchanged/doc46e/f39/hashes
restored, serviceactivePID11393/NRestarts0/no active journal. Full admission and
performance gates remain open.

Primary Black/Medium cycle56ebd92 succeeds after retained-descriptor change.
Exact helper SHA f0a94bed472965831c718c34d5b974e2f25f1f9f5dbfc8ad8e48a51c67684939;
ARM build1m56, 200 Windows-host tests on2682080 plus four focused owned-window
tests and strict checks on56. All25 injection windows reported positive delivery:
93..415events,44..137frames,83.723..221.657ms total window,2.763..6.676ms drain.
This verifies delivery on this native adapter; no EIO/reacquisition branch ran.
Acquisition358.423902ms, cleanup4995.625ms, final387.276ms: union5741.324902ms.
Cleanup preparation533.110ms and erase/verify4462.146ms; ten guards2681.086ms,
physical input1407.686ms, rearm65.750ms. Nested13 observations2529.721ms,
ten cleanup-pixel scopes133.475ms,118 input guards131.766ms,39 owner guards335.017ms
and checkpoint4.724ms are non-additive. This is one Medium sample, not a matched
Thin/Thick comparison, repeated latency gate, or full-workflow acceptance.
Stages202/333/398/407 new darkpixels stay within706..737/942..972; both tool tiles
remain exact, final corner clear, neighboring pixel rectangles exact.125 recognized
Lines equal before/after cycle; unknown native records remain outside parser proof.
V3 phases0/1/2 retired successfully; owner exact and no menu-input spans. Actual
primary Fine/Black/Medium unchanged, secondary untouched. Three developer sentinels
removed once along their recorded paths;122 original Lines exact after document
close. Original46e/f39 visually/native verified, notes/header/runtime hashes unchanged,
serviceactivePID12008/NRestarts0, no active journal. Installed runtime remains
caa5dd3e51e5443d665c727c4ff5730af475ec6d4d7175ef5eb3f78fab57f03b.
Production admission, remaining slots/negative/error/notes cases and full Q&A/history
remain open. Raw-event production observer replay is the next admission check.

Secondary Black/Thin cycle3c1dbd5 succeeds with native helper
SHA234834aee59275c0f0a2e40d631361dc195ce47bffc3844296f79603c2c87fa4 (ARM1m46).
Acquisition372.134213ms + cleanup5462.174ms + final393.716ms =6228.024213ms.
Cleanup preparation525.379ms; erase/verify4936.395ms. Ten guards3126.970ms,
physical input1426.747ms, rearm56.634ms. Nested13 observations2940.660ms and
pixel checks137.105ms are not additive.25 windows all reported positive input
(93..415events,44..137frames); no native EIO/reacquisition. Stages131/197/240/249
pixels within706..736/942..972, tool tiles exact, final clear, neighbor rectangles
exact, actual secondary Fine/Black/Thin unchanged, V3 journal completed/no menu spans.
The immediate source-drawn.rm still contained122 recognized Lines, while the
post-cycle file contained125: no original line missing or altered, exactly three
added two-point lines with the sentinel geometry. Therefore do not claim a125-to125
native comparison for this cycle. After one exact recorded-path sentinel erasure
and document close, all122 original Lines match exactly. Unknown native records
remain unverified. Restored temporarysecondaryFineMedium/originalsecondaryBallpoint
BlackMedium and selected unchanged primaryFine; original46e/f39 image+owner and
notes/header/runtime hashes verified, serviceactivePID12638/NRestarts0/no journal.
Remaining secondaryMedium, persistent failure codes, unsafe/unknown suppression,
notes/blank-context admission, normal no-menu wiring and full Q&A/history/latency
remain open. No final performance gate claimed from these single-width samples.

Secondary Black/Medium activity cycle3c1dbd5 succeeds: acquisition360.902387ms,
cleanup4934.595ms, final422.532ms, union5718.029387ms. Ten pathguards2686.756ms,
physical input1400.235ms, rearm50.214ms; nested13 observations2549.894ms, pixels
143.439ms.25 positive owned windows, no EIO/reacquisition.202/333/398/407 stagepixels
within706..737/942..972; exact tool tiles/neighbors and clear final corner. As for
Thin, immediate saved draw snapshot still122, postcycle125 with no original Line
changes and only the three owned sentinel lines. No125-to125 claim.

Same-source secondary Medium selection-failure case uses production draw_failure:
positive persistent X plus top horizontal code,220 new darkpixels707..737/943..973,
three positive windows(219..517events), acquisition371.982342ms/final384.616ms.
All125 pre-failure recognized Lines retained plus exactlythree mark Lines. Actual
FineBlackMedium unchanged, exact toolbar/neighbors/owner, journal completed without
menu input. Persistent marks have no automatic erasure/cleanup latency claim.
Developer erased only the three recorded X/code paths, then three sentinel paths.

Restoration finding: one e4 developer-helper post-tap screenshot refused two
framebuffer allocations. No repeated tap; an independent read-only capture confirmed
restored BallpointBlackMedium. This is not a candidate recovery-branch observation.
Initial post-cleanup screenshot looked clear, but the top horizontal sentinel remained
in the native file after close and a later read (123 Lines). Reopening displayed the
owned top line again. Fresh owner plus exact native coordinates justified ONE
deliberate same-path removal. After subsequent close,122 original recognized Lines
match exactly. Retain early snapshots; visible clearance and document close alone
are not proof of persisted erasure. No original Lines changed; unknown records are
outside parser proof. Original46e/f39 image/owner and notes/header/runtime hashes
verified, original secondaryBallpointBlackMedium/temporaryFineMedium restored,
primaryFine selected unchanged, serviceactivePID13465/NRestarts0/no active journal.
Remaining failure codes, negative/suppression, notes/blank pages, normal no-menu
integration, full Q&A/history and repeated latency gates remain open.

A511e10 primaryMedium native-snapshot cycle succeeds at the existing immediate
pixel/journal gates; durable saved-active-ink-to-clear proof is INCONCLUSIVE.
ARM1m47/helperSHA96bbd179b89160c4d007b41cab6e80557b9578c613d05c311180149ee415190a.
Initial/active/final native files are byte-identical SHA57472ec082f5a09c57736706802c8b15ac5a71d52dcf12da9b746204a88bb30e,
125 recognized baseline+sentinel Lines. Active screenshot positively shows407dark
marker pixels, but active snapshot at10.416s contains no marker Lines. Final
snapshot also old; later owner-bracketed polling yields different saved bytes
SHAc210849bf4c927679b37de786195becafdc52c22062c949ea29778f0715e2139 with the same125
recognized CRDT IDs/Line properties. Manualclose/reopen shows clear status corner,
all three neighbors intact and those same125 IDs/properties. No activity recurrence
in this case, but no persisted-active-marker proof. Do not relabel it a full gate.
Snapshots added11.569/10.772/15.258ms diagnostic overhead; timing union5767.62493ms
(acquire373.20293,cleanup4990.266,final404.156) is diagnostic-only, not a benchmark.
All standard stagepixels/tooltiles/neighbors remain exact; no EIO/reacquisition.

Manual cleanup control: each sentinel received ONE separate same-path erase, with
fresh owner-bracketed read-only saved-file observations before the next path. Stable
124/123/122-line results at18.250/16.594/17.297s include capture/SCP/three-sample
observation overhead, not erase latency. Each removed exactly the expected owned
line and preserved every other recognized property. After close/reopen, original122
CRDT IDs/properties match and no sentinel reappears. This differs from back-to-back
three-path cleanup, but does not isolate pacing from process/observation differences
or prove a minimum physical delay. No product pacing/erasure change.
Original46e/f39 verified visually/native, original notes/header/runtime hashes exact,
primaryFineBlackMedium unchanged/secondaryuntouched, serviceactivePID14387/NRestarts0,
no active journal. Remaining durable active-persistence, normal integration, all
negative/error/page/fullQ&A/history and performance gates stay open.

### Saved-active predicate refusal and manual recovery: bc692a5

Exact bc692a573ca57ae90364a32d522ab8a6281ced78 passed independent source review,
ARM example build1m29, host strict checks and six synthetic predicate tests.
Helper SHAe07f76e6bcb344ab21339d8ae75b230f2616ec7c8a9ad7ebba7c4a07898117a6;
host rmscene version0.8.0 verified. Additional offline tests using the actual saved
baseline rejected original parent, CRDT-left-link and opaque-value mutations.

ONE current-bc692-primary-medium-durable run saved125 original/sentinel lines plus
15 new owned marker lines. The host refused its ten-line expectation; device30s
proof deadline expired, retaining CurrentInk journal
90670009e42d4b988e332a5aec95098728c328b209da5cf66bb2e4cf47243662.
No ACK, production cleanup or journal retirement occurred in the diagnostic.
This is a diagnostic expectation/calibration failure, missed in initial review,
not a product erasure result. Initial/candidate bytes, full new line properties,
source/binary identity, screenshots, logs and journal are preserved.

Actual helper draws nine edges plus SIX repetitions of the final auxiliary circle;
Workflow retains ten unique cleanup paths. All nine native edges match their fixed
1-native-unit trajectory predicate. All six25-point native circle Lines are identical,
but fail that bound: bidirectional sampled trajectory distances1.252/1.554 native
units, endpoints0.378/0.585. Do not increase tolerance merely to fit this candidate
or claim raw point indices correspond to the emitted interpolation samples. A
bounded search of42 prior unique REM9/REM32 saved files found no independent saved
status-curve fixture. Geometry remains unresolved; another redraw is not yet useful.

Manual recovery was announced and separate: each of ten recorded owned paths was
erased once, then owner-bracketed saved observation reached125 original/sentinel
IDs/properties exactly. Close/reopen showed no marker recurrence. Each sentinel
then received one separate erase, with verified saved counts124/123/122 and exactly
one corresponding Line removed. Reopened source retained all122 original IDs/Line
properties and clear corner. Unknown native semantics are not claimed as verified.
Original46e/f39 document and unchanged actual tools restored; notes/header/runtime
hashes exact, serviceactive15902/NRestarts0. Original journal preserved and retired
only after manual restoration. No runtime installation or product admission.

### Saved-marker production cleanup and reopened verification: 556a599

Exact556a5992763e0fd136b3e57b054e7244e01f50bd independently reviewed; ARM example
build1m33, helperSHA6daa17b674952f11a6f7cf65285342237ba66f989fc9d23a359c9176c8954149.
Nine synthetic predicate regressions passed (mocked decode checks enforcement, not
parser fidelity). Original native-block mutation negatives were separately replayed.
ONE primaryBlackFineMedium cycle accepted saved active SHA
80f0b2503057b57bc62221f23886e5d71f4e5471bb29d46aa8b137a21b791ca3:
125 original/sentinel Lines plus15 marker IDs(1,1931..1945), all original full Line
blocks and non-PageInfo opaque records preserved. Circle acceptance is independently
measured ink-footprint equivalence, not exact commanded-centerline or vendor proof.

Unchanged guarded production cleanup completed, without toolbar input/reacquisition
or remaining journal. Immediate final native snapshot was still the old140 Lines;
it is not the final persistence proof. Later fresh owner-bracketed saved observations
reached125 Lines, SHA024cb5c94344b5824fd1e8ff4e153e651e35db9f5cffa6a6bf7daea40b707c6f.
ALL15 accepted additions absent and ALL125 original full SceneLineItemBlocks exact.
Manual close/reopen repeated those results and showed a clear corner with all three
neighboring sentinels present. Stage dark pixels202/333/398/407, bounds706942..737972;
actual tool tiles and neighboring rectangles exact, final clearance verified.

Raw audit: all66 prior opaque non-PageInfo records unchanged; one added record:
SceneLineItemBlock(extra_data=b'', parent_id=CrdtId(0,11),
item=CrdtSequenceItem(item_id=CrdtId(1,1931), left_id=CrdtId(1,1930),
right_id=CrdtId(0,0), deleted_length=19, value=None), extra_value_data=b'').
Four identifiers beyond the15 captured active markers have unobserved intermediate
provenance/semantics. No original live ID overlaps the corresponding1931..1949
interval, but that does not prove those four were eraser fragments. Claim only
preserved recognized original content/opaque records and durable removal of the15
accepted markers in this case; NOT full native-file equivalence/universal integrity.
No normalization of unexplained records or additional drawing to chase this detail.

Diagnostic waiting is separate from production timing: host handshake13.641s includes
its startup/transfer/validation, whole probe roughly33s. Acquisition367.347138ms,
cleanup4928.387ms, final restoration378.852075ms; nonoverlapping sum5674.586213ms.
Cleanup includes preparation554.386 and erase/verify4373.486; nested guards2599.011,
physical injection1460.574 and rearm59.240ms are not added to their parent totals.
Do not present the diagnostic as an end-to-end Q&A or repeated latency benchmark.

Three sentinels then received one erase each with saved verification124/123/122;
reopened source matches122 original IDs/properties with clear corner. Original46e/f39
visually/native verified; selected tools unchanged, notes/header/runtime hashes exact,
serviceactive17078/NRestarts0, no journal. Stop equivalent diagnostic batches here;
normal current-tool integration, suppression/error/notes cases and full Q&A/history,
performance/OpenSpec/PR/release gates remain open.
