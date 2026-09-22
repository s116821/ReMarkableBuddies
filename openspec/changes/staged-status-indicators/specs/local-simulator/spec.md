## MODIFIED Requirements

### Requirement: Observable indicator lifecycle

The simulator SHALL exercise production indicator eligibility and cleanup, record shared triangle/circle path state, repeated-stroke darkness, stage transitions, stroke times and ordered progress events, and support indicator faults. Temporary marks SHALL be distinct from persistent Q&A/line state. Captures, navigation and typing SHALL be asserted free of temporary marks, with deterministic scripted timing and separate explicit live wait tests. Source: src/simulator/device.rs; src/simulator/mod.rs; src/simulator/scenario.rs; tests/simulator.rs; tests/live_simulator.rs.

#### Scenario: Completion and failure
- **WHEN** scripted success, rejection, provider failure or recovery scenarios execute
- **THEN** reports expose ticks and cleanup ordering and no completed successful page retains a temporary mark.

#### Scenario: Existing content
- **WHEN** the corner contains an input sentinel
- **THEN** status is suppressed, the sentinel remains unchanged and normal Reader processing continues.

#### Scenario: Faulted cleanup
- **WHEN** a declared cleanup operation fails
- **THEN** assertions can detect the error, remaining mark state and absence of subsequent navigation/output.

#### Scenario: Geometry and cadence
- **WHEN** scripted stages and repeated pending ticks execute
- **THEN** the raster uses production geometry and virtual timestamps verify 333 ms stroke scheduling, stage completion and auxiliary-circle distinction.

#### Scenario: Failure code and partial stroke
- **WHEN** a classified failure or partial status-draw fault is injected
- **THEN** reports expose the appropriate persistent segment or bounded owned-path cleanup, and cleanup failure prevents navigation/output.


#### Scenario: Native erase has no visible effect
- **WHEN** StatusClear receives a no_move fault representing accepted input without visible cleanup
- **THEN** marks remain and the workflow stops before navigation or typing.
