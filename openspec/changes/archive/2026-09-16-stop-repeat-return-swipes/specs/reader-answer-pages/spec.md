## MODIFIED Requirements

### Requirement: Successor navigation and identity heuristic
After agreement, the system SHALL recapture the source, swipe left toward its immediate successor, wait and compare masked screenshots. Similarity at least 0.999 SHALL mean no navigation occurred, causing an X on the source without a reverse swipe. Source: src/workflow/orchestrator.rs render_answer; src/workflow/navigation.rs; src/workflow/xochitl_integration.rs.

#### Scenario: End of document
- **WHEN** the next-page attempt leaves the screenshot sufficiently similar to the source
- **THEN** the iteration draws the failure X and does not attempt to create a page or swipe backward.

#### Scenario: Fixed navigation timing
- **WHEN** the answer workflow requests next or previous navigation
- **THEN** the swipe uses 15 steps with 10 ms per step after a 50 ms initial contact, followed by 500 ms transition delay and an 800 ms settling wait.

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

### Requirement: Reusable page decisions and Q&A composition
The workflow SHALL expose reusable source-page verification and pure answer-page classification/Q&A composition helpers, preserving existing thresholds, masks, formatting and delays. Recovery SHALL use the single-attempt policy in Invalid successor recovery. Source: src/workflow/mod.rs, src/workflow/navigation.rs and src/workflow/orchestrator.rs.

#### Scenario: Equivalent navigation comparison
- **WHEN** forward movement or return-to-source is verified
- **THEN** both use the same masked source-page identity helper at threshold 0.999.

#### Scenario: Regression coverage
- **WHEN** rendering and classification regression tests run without device access
- **THEN** they cover blank/occupied/header-match decisions, UI-mask changes, different image sizes and exact Q&A separators/line breaks.
