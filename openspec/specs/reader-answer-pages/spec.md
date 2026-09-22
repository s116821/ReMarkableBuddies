# reader-answer-pages

## Purpose

Describe the implemented reader answer pages contracts, initially baselined from v0.1.4. Known gaps are explicit and require a later change delta to alter.
## Requirements

### Requirement: Successor navigation and identity heuristic
After agreement, the system SHALL recapture the source, swipe left toward its immediate successor, wait and compare masked screenshots. Similarity at least 0.999 SHALL mean no navigation occurred, causing an X on the source without a reverse swipe. Source: src/workflow/orchestrator.rs render_answer; src/workflow/navigation.rs; src/workflow/xochitl_integration.rs.

#### Scenario: End of document
- **WHEN** the next-page attempt leaves the screenshot sufficiently similar to the source
- **THEN** the iteration draws the failure X and does not attempt to create a page or swipe backward.

#### Scenario: Fixed navigation timing
- **WHEN** the answer workflow requests next or previous navigation
- **THEN** the swipe uses 15 steps with 10 ms per step after a 50 ms initial contact, followed by 500 ms transition delay and an 800 ms settling wait.

### Requirement: Blank and existing answer page classification
The classifier SHALL compare the current screenshot to white using masked grayscale similarity and classify at least 0.998 as Blank. Otherwise it SHALL compare the top 150 pixels with the cached header at threshold 0.998, classifying a match as ExistingQA and other pages as Invalid. Source: src/workflow/mod.rs is_valid_answer_page/compute_image_similarity_masked.

#### Scenario: Comparison masks
- **WHEN** full-page similarity is measured
- **THEN** left 298, right 125, top 70 and bottom 70 virtual pixels are excluded; header comparisons use bottom zero.
- **AND** mean squared grayscale differences determine similarity, sampling every fifth pixel normally and every second pixel for blank detection.

#### Scenario: Missing header cache
- **WHEN** a nonblank page has no readable matching cached pattern
- **THEN** it is Invalid; the app does not ask a model to classify its header.

### Requirement: Header and Q&A rendering
On Blank pages the system SHALL select body style, type === Reader Buddy Answers === with three newlines, wait 500 ms and attempt to cache the top-150-pixel header. On ExistingQA pages it SHALL omit the header and select body style. Both SHALL enclose each new block in `<Start of Q-A block for Q @ (x, y)>` and `<End of Q-A block for Q @ (x, y)>`, using identical normalized selected-content coordinates formatted to at most two decimal places without redundant trailing zeroes. Between these delimiter lines the system SHALL type Q: question, two newlines and A: answer; a newline SHALL precede the closing delimiter and terminate it. The former --- footer SHALL NOT be emitted for new blocks. Source: REM-31; REM-11 comments55ffdaa9/fedb3b45/71501e9a; src/workflow/mod.rs compose_qa and orchestrator.rs render_answer.

#### Scenario: Append
- **WHEN** the successor is recognized as ExistingQA
- **THEN** only the new delimited Q&A block is typed; its coordinate association belongs to its initial question, and no About entry or follow-up conversation is added. The successfully rendered block becomes the page-scoped last-Q&A transaction; existing content is excluded.

#### Scenario: Cache persistence
- **WHEN** the service restarts
- **THEN** /var/cache/reader-buddy/header-pattern.png is preserved.
- **AND** cache creation/save failures are logged and do not by themselves abort the workflow.

#### Scenario: Complete output boundary
- **WHEN** Q&A typing completes successfully
- **THEN** the exact composed block, including both delimiters and its final newline, is eligible for the shared undo/redo session, while a partial typing failure creates no usable history.

#### Scenario: Highlight and outline coordinates
- **WHEN** a highlighted or outlined selection has normalized center (0.5, 0.5)
- **THEN** both boundary lines contain Q @ (0.5, 0.5), while the question inside begins with ordinary Q:.

#### Scenario: Historical answers
- **WHEN** an existing page contains plain-Q/--- or Q-@/--- answers
- **THEN** appending a new delimited block preserves those existing answers exactly without migration, and does not claim to provide follow-up extraction or scrolled-page recognition.

### Requirement: Invalid successor recovery
An Invalid successor SHALL trigger a source-identity check before any reverse swipe. If already on the saved source at similarity 0.999, recovery SHALL not navigate. Otherwise it SHALL attempt at most one previous-page swipe and verify the result once, with no retry after failed verification or an input/capture error. The caller SHALL attempt the existing failure X on the current page after recovery or recovery failure. Source: src/workflow/navigation.rs; src/workflow/mod.rs return_to_original_page; src/workflow/orchestrator.rs render_answer.

#### Scenario: First return fails
- **WHEN** the single previous-page swipe does not restore source similarity
- **THEN** recovery reports Unconfirmed and the caller draws the failure X without another swipe.

#### Scenario: Already on the source
- **WHEN** recovery's initial check matches the saved source, including a forward attempt that did not leave it
- **THEN** recovery reports AlreadySource with no previous-page swipe.
- **AND** the existing forward no-movement guard still draws an X without invoking reverse recovery at end of document.

#### Scenario: Successful return
- **WHEN** the source is initially absent and the single previous-page swipe restores it
- **THEN** recovery reports Returned after one precheck, one swipe and one postcheck.

#### Scenario: Navigation or capture error
- **WHEN** a source-identity check or the previous-page operation returns an error
- **THEN** recovery propagates that error and performs no further navigation; the existing render-error handler attempts an X.

### Requirement: Failure display and loop errors

Every failure outcome SHALL obey guarded display policy: clear all temporary owned marks, then draw the constant X plus exactly one of six segments in a centered half-size box within the existing status region. The top edge SHALL mean unreadable/missing/ambiguous selection or malformed proposal; right edge transcription disagreement/invalid transcription; bottom edge provider unavailable/timeout/transport failure; left edge no successor movement; horizontal midpoint line unsuitable successor after confirmed recovery; vertical midpoint line device/render/recovery failure. Existing/unknown corner content SHALL suppress drawing and erasure. Cleanup failure SHALL stop further input. Failure marks SHALL persist, never enter the temporary ledger, and be attempted at most once per iteration. Render errors SHALL be logged and attempt the device code. Single-iteration provider/device errors SHALL propagate after guarded display; loop mode SHALL log them without typing arbitrary Error text into the document. Source: src/workflow/indicator.rs; src/workflow/mod.rs; src/workflow/orchestrator.rs.

#### Scenario: Proposal transport error
- **WHEN** a proposal request returns a provider error
- **THEN** temporary activity is cleared and the provider X/code is attempted safely before the error propagates; loop mode does not insert Error text.

#### Scenario: Failure after visible progress
- **WHEN** an eligible page has activity and the question is declined
- **THEN** all owned paths are erased before the constant X and selection-code top edge are drawn.

#### Scenario: Six distinct causes
- **WHEN** selection, transcription, provider, no-motion, invalid-successor or device failures occur on eligible pages
- **THEN** each uses its documented unique segment over the same X.

#### Scenario: Unconfirmed recovery
- **WHEN** invalid-successor recovery cannot confirm the saved source
- **THEN** the device/recovery code is attempted only if the current corner is known safe.

#### Scenario: Existing corner handwriting
- **WHEN** the corner is occupied before the iteration draws status marks
- **THEN** the failure mark is suppressed without erasing existing handwriting.

### Requirement: Reusable page decisions and Q&A composition
The workflow SHALL expose reusable source-page verification and pure answer-page classification/Q&A composition helpers, preserving existing thresholds, masks, formatting and delays. Recovery SHALL use the single-attempt policy in Invalid successor recovery. Source: src/workflow/mod.rs, src/workflow/navigation.rs and src/workflow/orchestrator.rs.

#### Scenario: Equivalent navigation comparison
- **WHEN** forward movement or return-to-source is verified
- **THEN** both use the same masked source-page identity helper at threshold 0.999.

#### Scenario: Regression coverage
- **WHEN** rendering and classification regression tests run without device access
- **THEN** they cover blank/occupied/header-match decisions, UI-mask changes, different image sizes and exact Q&A separators/line breaks.
