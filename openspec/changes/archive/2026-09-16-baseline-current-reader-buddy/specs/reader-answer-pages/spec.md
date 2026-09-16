## ADDED Requirements

### Requirement: Successor navigation and identity heuristic
After agreement, the system SHALL recapture the source, swipe left toward its immediate successor, wait and compare masked screenshots. Similarity at least 0.999 SHALL mean no navigation occurred, causing an X on the source without a reverse swipe. Source: src/workflow/orchestrator.rs render_answer; src/workflow/xochitl_integration.rs.

#### Scenario: End of document
- **WHEN** the next-page attempt leaves the screenshot sufficiently similar to the source
- **THEN** the iteration draws the failure X and does not attempt to create a page.

#### Scenario: Fixed navigation timing
- **WHEN** next or previous navigation is requested
- **THEN** the swipe uses 15 steps with 10 ms per step after a 50 ms initial contact, followed by 500 ms transition delay and the orchestrator's 800 ms settling wait.

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
On Blank pages the system SHALL select body style, type === Reader Buddy Answers === with three newlines, wait 500 ms and attempt to cache the top-150-pixel header. On ExistingQA pages it SHALL omit the header and select body style. Both SHALL type Q: question, two newlines, A: answer, then a newline-delimited --- separator. Source: src/workflow/orchestrator.rs render_answer.

#### Scenario: Append
- **WHEN** the successor is recognized as ExistingQA
- **THEN** only the new Q&A block is typed; no About entry, coordinate tag, undo transaction or follow-up session is added.

#### Scenario: Cache persistence
- **WHEN** the service restarts
- **THEN** /var/cache/reader-buddy/header-pattern.png is preserved.
- **AND** cache creation/save failures are logged and do not by themselves abort the workflow.

### Requirement: Invalid successor recovery
An Invalid successor SHALL cause up to three previous-page attempts, each verified against the saved source at similarity 0.999. After exhausting attempts the helper SHALL warn and return success; the caller SHALL draw an X on the current page. Source: src/workflow/orchestrator.rs return_to_original_page. This documents the existing REM-10 defect, not a recommendation.

#### Scenario: First return fails
- **WHEN** a previous-page swipe does not restore source similarity
- **THEN** the current implementation retries up to the three-attempt limit and can overshoot; no reliable native page identity is consulted.

### Requirement: Failure display and loop errors
Expected declines SHALL draw a 75 by 75 virtual-pixel X with 20-pixel bottom/right margin. Render-answer errors SHALL be logged and attempt an X. Unhandled iteration errors in loop mode SHALL attempt body-mode Error: text on the current page before continuing; single-iteration errors SHALL propagate. Source: src/workflow/mod.rs draw_failure_x; src/workflow/orchestrator.rs run_iteration/run_loop.

#### Scenario: Proposal transport error in loop mode
- **WHEN** a proposal request returns an error
- **THEN** the loop attempts error text on the currently active page, rather than guaranteeing an X-only failure.
- **AND** this baseline does not invoke the unused progress indicator helpers.
