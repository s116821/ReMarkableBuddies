## ADDED Requirements

### Requirement: History interaction events
While awaiting the next Reader action, tablet input SHALL recognize stationary four-contact holds of two seconds as undo and two-contact holds of two seconds as redo. Recognition SHALL use complete contact frames and stable tracking identities, emit once per hold, and require complete release before another action. The existing single-contact corner hold SHALL still initiate Reader. Departure or editing events SHALL invalidate history ownership without concurrent device input. Source: src/device/touch.rs event reducer and src/device/backend.rs idle interaction boundary.

#### Scenario: Four-contact hold
- **WHEN** four stable contacts remain stationary for two seconds
- **THEN** one undo event is emitted; transitional two-contact formation does not emit redo.

#### Scenario: Two-contact hold
- **WHEN** two stable contacts remain stationary for two seconds
- **THEN** one redo event is emitted without an additional conflicting native edit.

#### Scenario: Short or interrupted gesture
- **WHEN** a contact releases, moves, changes tracking identity or changes count before the threshold
- **THEN** no undo/redo event is emitted from the incomplete hold.

#### Scenario: Held contact and rearm
- **WHEN** a qualifying hold continues beyond two seconds
- **THEN** it does not repeat until all contacts have been released and a new qualifying hold begins.

#### Scenario: No history
- **WHEN** an undo/redo gesture occurs without a valid corresponding history state
- **THEN** Reader performs no text mutation and no model request.

#### Scenario: Corner release opens native menu
- **WHEN** a single contact qualifies for Reader and is subsequently released
- **THEN** the shared input path emits Reader once after release, so menu dismissal follows the native release action; no capture begins while the trigger is still held.

#### Scenario: History observer unavailable
- **WHEN** the native history observer cannot initialize
- **THEN** history is discarded and disabled for the process, while the ordinary Reader trigger remains available without a restart loop.

## MODIFIED Requirements

### Requirement: Corner hold trigger
The shared history-capable input path SHALL recognize one stationary contact in the configured 68-pixel corner region of the virtual 768 by 1024 screen after a continuous two-second hold and full release. The legacy fallback SHALL retain its slot-zero two-second trigger. Both paths SHALL evaluate complete input frames and support stationary holds without new position events. Source: src/device/interaction.rs; src/device/native_history.rs; src/device/touch.rs.

#### Scenario: Stationary hold
- **WHEN** a valid single contact stays in the selected corner for two seconds and releases
- **THEN** the shared path recognizes Reader once without requiring movement, and Workflow injects its middle-bottom dismissal tap before capture.

#### Scenario: Contact ends or exits
- **WHEN** contact ends before two seconds or its complete frame leaves the trigger region
- **THEN** the hold is canceled and a later valid hold starts fresh.

#### Scenario: Legacy fallback
- **WHEN** the shared native history observer is unavailable
- **THEN** the existing slot-zero polling trigger remains available at its two-second threshold, without native history ownership.

### Requirement: Deterministic hold timing
Hold timing SHALL be isolated from Linux polling so contact loss, leaving/reentering, stationary timeout and release ordering can be tested without device access. The two-second threshold SHALL remain unchanged. Source: src/device/touch.rs; src/device/interaction.rs.

#### Scenario: Interrupted hold
- **WHEN** contact exits the zone or ends before timeout
- **THEN** its elapsed time is discarded and the next valid hold starts fresh.

#### Scenario: No movement required
- **WHEN** a valid contact remains stationary until the threshold
- **THEN** no new position event is required for qualification; the shared path emits Reader on full release, while the legacy timer retains threshold activation.
