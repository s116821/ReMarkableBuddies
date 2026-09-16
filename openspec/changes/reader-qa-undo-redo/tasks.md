## 1. Native feasibility

- [ ] 1.1 Add bounded diagnostic actions and verify exact native Q&A range deletion/restoration with header, prior answers, newlines and nearby ink; record chosen mechanism and revise design from evidence.
- [ ] 1.2 Observe current-page/departure/edit events, including fast leave-and-return and alternate navigation, and validate two-/four-contact long holds against native gestures; establish a reliable fail-closed ownership policy.

## 2. Shared implementation

- [x] 2.1 Implement pure last-Q&A history state and invalidation/failure policy with focused preservation tests.
- [ ] 2.2 Implement multi-contact event reduction, hold timing/rearm and idle interaction events without changing ordinary Reader triggering.
- [ ] 2.3 Integrate verified native block editing, ownership checks and orchestrator transaction registration/invalidation; keep all device input serialized.
- [x] 2.4 Extend simulator session actions, text/history reports and faults; cover repeated toggles, blank/append preservation, departure/return, new iteration, edits and partial failures.

## 3. Verification and delivery

- [ ] 3.1 Pass appropriate tests, formatting, strict native/ARM lint, both ARM builds and explicit model regression; distinguish scripted/local HTTP/live/native evidence.
- [ ] 3.2 Verify final native undo/redo sessions and Reader regression on a disposable technical paper, then restore original document/cache/tool/service state.
- [ ] 3.3 Verify requirements/design coverage, update docs, sync canonical specifications and archive this change in the implementation PR.

Fresh independent final-head review and green CI/automated review remain merge gates.
