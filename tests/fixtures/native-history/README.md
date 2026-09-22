# Native text preservation fixtures

RM2 firmware3.28.0.172, disposable PDF notes page. These RMv6 files contain typed Q&A and native scene records, not the source PDF. Short applied/removed/restored fixtures represent one exact36-character selection deletion and the anchored native CtrlZ restore. Long fixtures represent328characters. Files were retrieved after native persistence settled; immediate snapshots were observed to lag the UI and are deliberately excluded.

Expected properties: older text and paragraph/inline styles preserved; restored complete text/styles identical; native scene records and root layout bytes preserved. These fixtures validate read-only extraction and state comparison, not permission to issue keyboard undo. The owning process/session, page revision and input/event guards remain separate requirements.

Reference extraction: rmscene0.8.0 (MIT), https://github.com/ricklupton/rmscene. Root trailing18bytes and extra SceneInfo fields are not semantically interpreted by this diagnostic reader. Tests preserve them byte-for-byte; successful extraction alone must not authorize editing on unknown schemas.

The `ink-*` sequence adds a visually confirmed diamond beside earlier answers before appending another36-character block. Its eight native line records remain identical across before/applied/deleted/restored; all23 prior visible paragraphs remain exact. The empty insertion paragraph changes style1 to3 through body mode, while restored text/styles exactly equal applied. This is an observed exception for the empty insertion paragraph, not permission to change prior visible formatting. The history-policy regression replays these native snapshots without emitting input.

inline-applied.rm / inline-partial.rm preserve the failed fullReader firstQA test onRM2 firmware3.28.0.172. Fast5ms character selection passed in expandedEdittext UI but lost keys in the normalinline editor: deletion lefttheQlineandA:G whilepreservingheader. The production adapter rejected the incomplete result and discardedhistory. This fixture must not be relabeled as successful undo; the simulator partial fault models its no-compensation policy without pretending to reproduce native key-delivery timing.

paragraph-applied/deleted/restored.rm record the next bounded normal-inline probe. From the visually verified insertion cursor, fourCtrlShiftUp steps selectedexactlythe4paragraphQ&A; the selection was visuallyinspectedbeforeBackspace. Headeronlyremained31characters; nativeCtrlZ restoredexacttext/styles. Sharedfixturetestchecksallnontextrecordsandheaderpreservation. Laterintegrated/captestsareseparateevidence.

operators-reordered.rm plusoperators-expected.txt preserve a32paragraphinline stresscase: renderedcaretcompositionreorderedseveralx^2sequences(e.g.^x2). Nativehistoryrefusedtoarmbecauseactualcompletecontentdiffered; nodeletionwasattempted. Timingfixrequiresrealdeviceverification,notjustfixturetests.

tagged-applied/deleted/restored.rm record the REM-11 live highlighted-question cycle on RM2 firmware3.28.0.172, runtime9bf7757. Four simultaneous synthetic contacts held2200ms removed the complete Q @ (0.55, 0.24) block; two contacts restored it. The read-only production-parser regression checks exact header-only removal and restored text/styles, scene records and root layout. These snapshots do not establish physical-finger or model-localization accuracy.

tagged-append-scene-change.rm preserves a later installed-service append whose history did not act. Compared with the prior completed tagged-restored snapshot, it adds native scene records. The replay verifies that this changed scene cannot claim ownership using the older snapshot. It does not prove the precise live pre-typing snapshot or attribute those records to a particular UI action; activity-drawing persistence is a hypothesis investigated separately.

delimited-before/applied/deleted/restored.rm preserve the REM-31 offline native
append cycle on RM2 firmware3.28.0.172, source89f72b8. A prior live-model answer
with matching Start/End delimiters remains intact while a second deterministic
block is appended, deleted and restored twice. The before snapshot is immediately
before the diagnostic; later files were collected only after the production
history adapter reported settled Applied/Undone states. The replay verifies exact
production composition, five owned newline paragraphs, header/prior text/styles,
scene records and root layout. Separate early gesture snapshots lagged the visible
screen because persistence had not settled; they remain local evidence and are
not presented here as settled native transaction fixtures. These captures do not
establish physical-finger performance or resolve unrelated indicator/scene changes.
