## ADDED Requirements
### Requirement: Responsiveness state and timing regressions
The simulator SHALL model changed operation timing and reachable delayed/stale/changed-state outcomes in the same implementation PR, preserving exact output, forbidden writes/navigation, history ownership and bounded failure behavior. Simulated time SHALL be labeled modeled time rather than native latency. Source: REM9.

#### Scenario: Faster path with delayed observation
- **WHEN** a changed workflow sees delayed convergence or a changed page
- **THEN** it either verifies a fresh eligible state within its bound or refuses safely without stale input, preserving existing negative assertions.

#### Scenario: History remains free of status switching
- **WHEN** undo or redo is exercised after a complete answer
- **THEN** its exact content and ownership rules remain unchanged and no new status tool acquisition or indicator loop occurs.

#### Scenario: Completion signal ordering and cancellation
- **WHEN** production sequencing receives immediate, delayed, missing, duplicate, out-of-order, stale or wrong-owner signals, or cancellation
- **THEN** modeled checks assert fresh correlated completion or bounded safe failure, no duplicate mutations and no late revival of cancelled work; timed simulation is not proof of native event availability.

### Requirement: Current-tool drawing regressions
The simulator SHALL exercise the production current-tool eligibility and cleanup
path without pretending modeled tool footprints establish native safety.

#### Scenario: No simulated tool switching
- **WHEN** a supported current-tool indicator or a suppressed unsuitable-tool case runs
- **THEN** the operation records zero toolbar selection/menu presses and unchanged original tool settings; unsupported feedback does not prevent core Q&A.

#### Scenario: Footprint and recovery faults
- **WHEN** a stroke or eraser envelope approaches neighboring ink, or owner/input/journal/cleanup observations fail
- **THEN** unsupported tool/layout eligibility suppresses optional ink; owner/input/capture/journal errors fail closed even before drawing, and post-mutation uncertainty retains recovery evidence; no broadened erasure or stale success occurs.

#### Scenario: Declared unsupported status capability
- **WHEN** a page declares an unsuitable tool, unknown tool or unverified layout
- **THEN** the shared workflow preserves exact core Q&A with no status drawing or erasure; this declaration is a modeled input and does not prove native recognition or notes-page eligibility.

#### Scenario: Conditional trigger dismissal
- **WHEN** a modeled hold qualifies
- **THEN** a declared closed overlay emits no tap, a qualified known-open panel receives one outside-panel tap, and unknown or failed dismissal stops before Q&A mutations; actual native overlay behavior remains a separate acceptance gate.
