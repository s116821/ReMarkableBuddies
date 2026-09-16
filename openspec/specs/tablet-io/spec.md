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
- **THEN** it requires exactly one matching 32-bit mmap allocation header for 1404 by 1872 by four bytes and reads pixels after the eight-byte header.

#### Scenario: Native detail and overview
- **WHEN** modern RM2 pixels are encoded
- **THEN** the blue channel supplies full-range grayscale in portrait 1404 by 1872 without legacy rotation.
- **AND** the API provides a 768 by 1024 overview plus three overlapping full-width native-detail strips.

#### Scenario: Legacy and Paper Pro branches
- **WHEN** another implemented capture branch is selected
- **THEN** legacy RM2 uses its existing conversion/rotation/flip and Paper Pro uses its existing four-byte 1632 by 2154 path.
- **AND** presence of these paths does not establish hardware compatibility for every firmware.

### Requirement: Corner hold trigger
The trigger SHALL use slot-zero touch coordinates evaluated at SYN_REPORT in a configured 68-pixel corner region of the virtual 768 by 1024 screen. It SHALL activate after a continuous two-second hold, including a stationary hold with no subsequent position events. Source: src/device/touch.rs wait_for_trigger/is_in_trigger_zone.

#### Scenario: Stationary hold
- **WHEN** a valid slot-zero contact stays in the selected corner for two seconds
- **THEN** the 10 ms polling loop recognizes the trigger without requiring movement.
- **AND** Workflow then injects a 100 ms middle-bottom tap before capture.

#### Scenario: Contact ends or exits
- **WHEN** contact ends or its complete input frame leaves the trigger region
- **THEN** the hold timer resets.
- **AND** this baseline has no centralized multi-finger gesture arbitration or release debounce contract.

### Requirement: Coordinate and output assumptions
Input and workflow operations SHALL use virtual portrait coordinates 768 by 1024 with device-specific transformations; RM2 touch Y is inverted and Paper Pro touch is directly scaled. Pen and keyboard output SHALL use Linux input injection, not direct native document edits. Source: src/device/touch.rs virtual_to_input; src/device/pen.rs; src/device/keyboard.rs.

#### Scenario: Body text output
- **WHEN** a Q&A is written
- **THEN** the keyboard selects body style with Ctrl+3 and emits supported character key events, including the special caret handling in string_to_keypresses.

#### Scenario: Coverage boundary
- **WHEN** assessing compatibility
- **THEN** the recorded RM2 3.28.0.172 portrait evidence is distinguished from untested human gestures, other orientations, reboot behavior and Paper Pro hardware; non-Linux stubs are not a faithful simulator.

### Requirement: Deterministic hold timing
The hold timer SHALL be isolated from the Linux polling loop so contact loss, leaving/reentering the corner and stationary timeout behavior can be tested without device access. The two-second threshold and slot-zero event routing SHALL remain unchanged. Source: src/device/touch.rs.

#### Scenario: Interrupted hold
- **WHEN** contact exits the zone or ends before timeout
- **THEN** its elapsed time is discarded and the next valid hold starts fresh.

#### Scenario: No movement required
- **WHEN** a valid contact stays stationary until the threshold
- **THEN** polling the timer activates exactly at two seconds without new position events.
