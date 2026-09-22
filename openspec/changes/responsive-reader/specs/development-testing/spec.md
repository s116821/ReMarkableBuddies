## ADDED Requirements
### Requirement: Completion-driven workflow guidance
Repository AGENTS/workflow guidance SHALL establish supported completion events plus verified postconditions, otherwise bounded fresh-state polling with deadlines/cancellation, as the default for future Reader/Writer features and improvements. Necessary fixed-wait exceptions SHALL include reason, scope, bound and validation; gesture/cadence/timeout behavior SHALL be distinguished from completion assumptions. Public/manual contributor workflows SHALL remain supported without a private integration or invasive event dependency. Source: REM9 and REM35 September22 architecture steering.

#### Scenario: Future feature introduces a wait
- **WHEN** a contributor plans sequencing for a new operation
- **THEN** the change records the required completion condition, supported signal or bounded polling fallback, owner correlation and tests for delayed/lost/stale/repeated signals and cancellation.

#### Scenario: Integrated final gate
- **WHEN** REM35 validates later Reader/Writer features
- **THEN** it audits new waits and verifies preservation of completion-driven sequencing, safety guarantees and REM9 responsiveness gains before1.0.
