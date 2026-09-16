## ADDED Requirements

### Requirement: Last successful Q&A transaction
Reader SHALL retain only the most recent completely rendered Q&A block and its answer-page identity in process memory. The record SHALL exclude the header, older answers and unrelated ink. Undo SHALL remove exactly that block; redo SHALL restore exactly that block without a model call. Source: planned src/workflow/history.rs; src/workflow/orchestrator.rs; src/device/backend.rs.

#### Scenario: Blank answer page
- **WHEN** the first Q&A is undone after successful rendering
- **THEN** its header and unrelated ink remain, and redo restores the same Q&A once.

#### Scenario: Existing answer page
- **WHEN** a newly appended Q&A is undone and redone repeatedly
- **THEN** all earlier answers remain unchanged and the latest block alternates between absent and present without duplication.

#### Scenario: Failed render or mutation
- **WHEN** rendering or undo/redo fails or its resulting state is uncertain
- **THEN** the history record is unavailable, the failure is reported and no automatic compensating edit is attempted.

### Requirement: Page-scoped history ownership
Reader SHALL invalidate the saved record when the user departs the answer page or a new Reader iteration starts. Leaving and returning SHALL NOT revive it. Every mutation SHALL check current ownership; intervening edits or uncertain identity SHALL disable it rather than delete unrelated content. Source: planned workflow history policy and device event/identity backend.

#### Scenario: Departure and return
- **WHEN** the user leaves the answer page and later returns
- **THEN** neither undo nor redo can use the old record, including after a rapid leave-and-return.

#### Scenario: New iteration
- **WHEN** another Reader iteration begins
- **THEN** the old record is invalidated before model processing, even if the new request is declined.

#### Scenario: Changed or unknown content
- **WHEN** the current page or its text no longer matches owned state, or observation fails
- **THEN** the requested mutation does not run and the record is discarded.

#### Scenario: Restart
- **WHEN** the service restarts
- **THEN** it has no undo/redo transaction from the previous process.
