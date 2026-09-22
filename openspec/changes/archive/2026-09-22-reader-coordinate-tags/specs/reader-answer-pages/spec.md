## MODIFIED Requirements

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

