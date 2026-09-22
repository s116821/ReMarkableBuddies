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


## ADDED Requirements

### Requirement: Status style lifecycle and rollback model

The local test suite SHALL model status-style acquisition, unavailability and restoration faults, and reject capture/navigation/typing/history while a style lease remains active. A deterministic toolbar state model SHALL exercise exact primary/secondary preference restoration, partial acquisition actions, page changes and pre-mutation layout refusal. Native screenshot fixtures SHALL check the supported layout classifier. These models SHALL NOT claim native UI timing, persistence convergence or legibility proof.

#### Scenario: Restoration fails
- **WHEN** the native style restoration is modeled to fail after status cleanup
- **THEN** further page input and later iterations stop rather than treating cleared ink as restored user preferences.

#### Scenario: Unsupported controls
- **WHEN** style acquisition reports an unsupported layout
- **THEN** status is suppressed without repeatedly toggling controls or preventing model analysis.


#### Scenario: Stale preference file during style probing
- **WHEN** actual UI settings differ from advisory persisted preferences and an input fails during probing, setting or restoration
- **THEN** modeled rollback uses durably captured UI values only, restores only potentially changed dimensions, and stops after unverified restoration instead of copying the stale file's values.
