## MODIFIED Requirements

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
