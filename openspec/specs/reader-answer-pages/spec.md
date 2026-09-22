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
On Blank pages the system SHALL select body style, type === Reader Buddy Answers === with three newlines, wait 500 ms and attempt to cache the top-150-pixel header. On ExistingQA pages it SHALL omit the header and select body style. Both SHALL type Q @ (x, y): question with normalized selected-content coordinates formatted to at most two decimal places without redundant trailing zeroes, two newlines, A: answer, then a newline-delimited --- separator. Source: src/workflow/orchestrator.rs render_answer.

#### Scenario: Append
- **WHEN** the successor is recognized as ExistingQA
- **THEN** only the new Q&A block is typed; the coordinate tag belongs to the new Q&A, and no About entry or follow-up conversation is added. The successfully rendered block becomes the page-scoped last-Q&A transaction; existing content is excluded.

#### Scenario: Cache persistence
- **WHEN** the service restarts
- **THEN** /var/cache/reader-buddy/header-pattern.png is preserved.
- **AND** cache creation/save failures are logged and do not by themselves abort the workflow.

#### Scenario: Complete output boundary
- **WHEN** Q&A typing completes successfully
- **THEN** the exact composed block is eligible for the shared undo/redo session, while a partial typing failure creates no usable history.

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

Every failure-X outcome, including analysis rejection, navigation no-motion and recovery, SHALL obey this guarded display policy. Expected declines SHALL attempt an X within the same 50 by 50 virtual-pixel bottom-right status region used by the activity circle, after clearing any owned circle. If preexisting content makes status drawing unsafe, the failure mark SHALL be suppressed without erasing that content. Render-answer errors SHALL be logged and attempt an X after cleanup. Unhandled iteration errors in loop mode SHALL attempt body-mode Error: text on the current page after cleanup before continuing; single-iteration errors SHALL propagate. Source: src/workflow/mod.rs draw_failure_x; src/workflow/indicator.rs; src/workflow/orchestrator.rs run_iteration/run_loop.

#### Scenario: Proposal transport error in loop mode
- **WHEN** a proposal request returns an error
- **THEN** the loop attempts error text on the currently active page after indicator cleanup, rather than guaranteeing an X-only failure.

#### Scenario: Failure after visible progress
- **WHEN** an eligible page has a circle and the question is declined
- **THEN** the circle is erased before the two failure-X diagonals are drawn within the 50 by 50 region.

#### Scenario: Existing corner handwriting
- **WHEN** the corner is occupied before the iteration draws status marks
- **THEN** the failure mark is suppressed rather than erasing preexisting handwriting.

### Requirement: Reusable page decisions and Q&A composition
The workflow SHALL expose reusable source-page verification and pure answer-page classification/Q&A composition helpers, preserving existing thresholds, masks, formatting and delays. Recovery SHALL use the single-attempt policy in Invalid successor recovery. Source: src/workflow/mod.rs, src/workflow/navigation.rs and src/workflow/orchestrator.rs.

#### Scenario: Equivalent navigation comparison
- **WHEN** forward movement or return-to-source is verified
- **THEN** both use the same masked source-page identity helper at threshold 0.999.

#### Scenario: Regression coverage
- **WHEN** rendering and classification regression tests run without device access
- **THEN** they cover blank/occupied/header-match decisions, UI-mask changes, different image sizes and exact Q&A separators/line breaks.
