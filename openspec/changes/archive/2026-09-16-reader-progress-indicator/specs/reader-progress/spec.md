## ADDED Requirements

### Requirement: Safe visible activity

After trigger and clean page capture, Reader SHALL display a recurring circle within a 50 by 50 virtual-pixel bottom-right region while model processing is pending, and show activity at safe stages on each visited page. All device operations SHALL remain serialized on the workflow thread. Source: src/workflow/indicator.rs; src/workflow/mod.rs; src/device/backend.rs; src/device/pen.rs.

#### Scenario: Pending model response
- **WHEN** a model request remains pending on an eligible page
- **THEN** the circle is refreshed periodically without concurrent pen, touch or keyboard input.

#### Scenario: Occupied status region
- **WHEN** the status region or required clearance contains preexisting content, or eligibility is unknown
- **THEN** Reader suppresses the indicator and its erasure while continuing question processing.
- **AND** it does not restore native document ink by merely repainting screenshot pixels.

### Requirement: Owned-mark cleanup

Reader SHALL track whether it owns a temporary status mark and clear it before captures, navigation, keyboard output, successful completion or failure display. Cleanup SHALL run on provider error/timeout, rejection and rendering/navigation errors. A failed clear SHALL prevent subsequent navigation and be reported. Source: src/workflow/indicator.rs; src/workflow/mod.rs; src/workflow/orchestrator.rs.

#### Scenario: Successful navigation and output
- **WHEN** an accepted question moves from source to successor and writes an answer
- **THEN** the source has no temporary circle before the swipe, model/classifier/header captures contain no circle and successful completion leaves no circle on the answer page.

#### Scenario: Rejection or provider failure
- **WHEN** no readable question is found, transcription disagrees, or a model request fails or times out
- **THEN** the temporary circle is cleared before failure display or error propagation.

#### Scenario: Cleanup input failure
- **WHEN** clearing an owned mark fails
- **THEN** the workflow reports the error and does not proceed to navigation or write the model answer.
- **AND** it does not claim that the native mark was removed.

#### Scenario: Uncertain page after cleanup or typing failure
- **WHEN** cleanup fails or partial typing may have changed the corner
- **THEN** failed cleanup prevents further iterations on the same orchestrator, and status eligibility after typing is unknown until another clean capture.

#### Scenario: Invalid successor recovery takes priority
- **WHEN** the clean successor capture classifies the page as invalid
- **THEN** Reader attempts the verified return and source failure display without first drawing an activity circle on the invalid successor.
