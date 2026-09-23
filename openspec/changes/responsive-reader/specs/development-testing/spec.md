## ADDED Requirements
### Requirement: Completion-driven workflow guidance
Repository AGENTS/workflow guidance SHALL establish supported completion events plus verified postconditions, otherwise bounded fresh-state polling with deadlines/cancellation, as the default for future Reader/Writer features and improvements. Necessary fixed-wait exceptions SHALL include reason, scope, bound and validation; gesture/cadence/timeout behavior SHALL be distinguished from completion assumptions. Public/manual contributor workflows SHALL remain supported without a private integration or invasive event dependency. Source: REM9 and REM35 September22 architecture steering.

#### Scenario: Future feature introduces a wait
- **WHEN** a contributor plans sequencing for a new operation
- **THEN** the change records the required completion condition, supported signal or bounded polling fallback, owner correlation and tests for delayed/lost/stale/repeated signals and cancellation.

#### Scenario: Integrated final gate
- **WHEN** REM35 validates later Reader/Writer features
- **THEN** it audits new waits and verifies preservation of completion-driven sequencing, safety guarantees and REM9 responsiveness gains before1.0.

### Requirement: Predictable menu-free product interaction
Reader, Writer and future features SHALL NOT autonomously navigate device menus,
including opening menus to inspect state. They SHALL prefer supported direct
interfaces, verified simple gestures or documented safe fallback. Simple left/right
page swipes and keyboard text input remain permitted under their existing guards.

#### Scenario: Feature requires a menu
- **WHEN** no supported non-menu mechanism can implement a proposed feature
- **THEN** the limitation and design choice are surfaced explicitly instead of introducing hidden autonomous menu navigation.

#### Scenario: Developer setup
- **WHEN** deliberate developer/manual validation setup uses a menu
- **THEN** it is separated from normal product execution, announced and labeled as setup; it is not evidence that normal product behavior is menu-free.

#### Scenario: Integrated audit
- **WHEN** REM35 validates the integrated Reader and Writer release
- **THEN** every normal feature path is audited for menu automation, with permitted gestures and keyboard input distinguished explicitly.
