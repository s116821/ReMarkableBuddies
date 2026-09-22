# tablet-io

## Purpose

Describe the implemented tablet io contracts, initially baselined from v0.1.4. Known gaps are explicit and require a later change delta to alter.

## Requirements

### Requirement: Device selection
The system SHALL detect RM2 from reMarkable2 1.0 and Paper Pro from ferrari 1.0 in /etc/hwrevision; other values SHALL select Unknown with legacy RM2 defaults in device implementations. Source: src/device/mod.rs; src/device/{screenshot,touch,pen}.rs.

#### Scenario: Linux input selection
- **WHEN** RM2 is detected
- **THEN** pen input uses /dev/input/event1 and touch uses /dev/input/event2.
- **AND** Paper Pro instead selects event2 for pen and event3 for touch.

### Requirement: Screenshot layout and ambiguity handling
RM2 SHALL select four-byte BGRA for IMG_VERSION major/minor at least 3.24 and the legacy two-byte layout otherwise. Missing or malformed RM2 version fields SHALL error. Capture SHALL read xochitl process memory and reject missing or ambiguous modern RM2 allocations. Source: src/device/screenshot.rs rm2_uses_bgra/find_rm2_bgra_allocation/take_screenshot.

#### Scenario: Modern RM2 allocation
- **WHEN** modern RM2 capture searches anonymous writable mappings
- **THEN** it searches page-aligned addresses where the complete allocation fits, requires exactly one matching 32-bit mmap allocation header for 1404 by 1872 by four bytes, and reads pixels after the eight-byte header.

#### Scenario: Merged memory mappings after restart
- **WHEN** a valid framebuffer allocation is inside a merged anonymous mapping rather than at its start
- **THEN** discovery finds its validated header without a fixed address or historical-offset fallback.
- **AND** missing, malformed, truncated and multiple matching allocations are not accepted as a unique framebuffer.

#### Scenario: Native detail and overview
- **WHEN** modern RM2 pixels are encoded
- **THEN** neutral-preserving luminance from the BGR channels supplies full-range grayscale in portrait 1404 by 1872 without legacy rotation, retaining text contrast in colored highlights.
- **AND** the API provides a 768 by 1024 overview plus three overlapping full-width native-detail strips.

#### Scenario: Legacy and Paper Pro branches
- **WHEN** another implemented capture branch is selected
- **THEN** legacy RM2 uses its existing conversion/rotation/flip and Paper Pro uses its existing four-byte 1632 by 2154 path.
- **AND** presence of these paths does not establish hardware compatibility for every firmware.


#### Scenario: Colored highlighter pixels
- **WHEN** modern RM2 native pixels contain a yellow highlight over printed text
- **THEN** grayscale conversion uses (77R+150G+29B+128)>>8, preserving neutral values exactly and keeping yellow background lighter than black text.

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

### Requirement: Coordinate and output assumptions
Input and workflow operations SHALL use virtual portrait coordinates 768 by 1024 with device-specific transformations; RM2 touch Y is inverted and Paper Pro touch is directly scaled. Pen and keyboard output SHALL use Linux input injection, not direct native document edits. Source: src/device/touch.rs virtual_to_input; src/device/pen.rs; src/device/keyboard.rs.

#### Scenario: Body text output
- **WHEN** a Q&A is written
- **THEN** the keyboard selects body style with Ctrl+3 and emits supported character key events, including the special caret handling in string_to_keypresses.

#### Scenario: Coverage boundary
- **WHEN** assessing compatibility
- **THEN** the recorded RM2 3.28.0.172 portrait evidence is distinguished from untested human gestures, other orientations, reboot behavior and Paper Pro hardware; non-Linux stubs are not a faithful simulator.

### Requirement: Deterministic hold timing
Hold timing SHALL be isolated from Linux polling so contact loss, leaving/reentering, stationary timeout and release ordering can be tested without device access. The two-second threshold SHALL remain unchanged. Source: src/device/touch.rs; src/device/interaction.rs.

#### Scenario: Interrupted hold
- **WHEN** contact exits the zone or ends before timeout
- **THEN** its elapsed time is discarded and the next valid hold starts fresh.

#### Scenario: No movement required
- **WHEN** a valid contact remains stationary until the threshold
- **THEN** no new position event is required for qualification; the shared path emits Reader on full release, while the legacy timer retains threshold activation.

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
