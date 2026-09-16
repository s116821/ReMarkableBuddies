## ADDED Requirements

### Requirement: Observable indicator lifecycle

The simulator SHALL exercise production indicator eligibility and cleanup, record temporary circle state and ordered progress events, and support indicator faults. Temporary marks SHALL be distinct from persistent Q&A/line state. Captures, navigation and typing SHALL be asserted free of temporary circles, with deterministic scripted timing and separate explicit live wait tests. Source: src/simulator/device.rs; src/simulator/mod.rs; src/simulator/scenario.rs; tests/simulator.rs; tests/live_simulator.rs.

#### Scenario: Completion and failure
- **WHEN** scripted success, rejection, provider failure or recovery scenarios execute
- **THEN** reports expose ticks and cleanup ordering and no completed successful page retains a temporary circle.

#### Scenario: Existing content
- **WHEN** the corner contains an input sentinel
- **THEN** status is suppressed, the sentinel remains unchanged and normal Reader processing continues.

#### Scenario: Faulted cleanup
- **WHEN** a declared cleanup operation fails
- **THEN** assertions can detect the error, remaining mark state and absence of subsequent navigation/output.
