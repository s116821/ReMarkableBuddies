## MODIFIED Requirements

### Requirement: Safe visible activity

After trigger and clean page capture, Reader SHALL draw nested Preparing, AnswerPending and AnswerReady triangles within the existing 50 by 50 virtual-pixel bottom-right region. Preparing precedes answer dispatch, AnswerPending starts at dispatch, and AnswerReady follows the response. Each stage SHALL complete its first three-edge traversal before advancement. Stroke starts SHALL target 333 ms intervals accounting for draw duration, skipping missed deadlines without bursts. Pending stages SHALL repeatedly trace their edges. Auxiliary inference SHALL use a refreshed incircle tangent to the innermost displayed triangle, retaining the triangle paths. All device operations SHALL remain serialized on the workflow thread. Source: src/workflow/indicator.rs; src/workflow/mod.rs; src/device/backend.rs; src/device/pen.rs.

#### Scenario: Pending model response
- **WHEN** a model request remains pending on an eligible page
- **THEN** the current triangle is retraced edge by edge at the requested cadence without concurrent pen, touch or keyboard input.

#### Scenario: Occupied status region
- **WHEN** the status region or required clearance contains preexisting content, or eligibility is unknown
- **THEN** Reader suppresses the indicator and its erasure while continuing question processing.
- **AND** it does not restore native document ink by merely repainting screenshot pixels.

#### Scenario: Fast stage transition
- **WHEN** an answer response completes before its triangle traversal finishes
- **THEN** missing first-pass edges finish at the same cadence before the smaller AnswerReady triangle is drawn.

#### Scenario: Auxiliary transcription
- **WHEN** independent transcription is pending
- **THEN** the AnswerReady triangle remains and its tangent incircle is refreshed without another device writer.

#### Scenario: Slow device stroke
- **WHEN** drawing exceeds a scheduled interval
- **THEN** the next deadline moves forward without accumulating a burst of missed strokes.

#### Scenario: Undo or redo
- **WHEN** a history gesture is processed
- **THEN** no activity stage or auxiliary circle is drawn.


### Requirement: Owned-mark cleanup

Reader SHALL record each unique owned path before drawing, including possible partial strokes, in a finite ledger and clear it before captures, navigation, keyboard output, successful completion or failure display. Cleanup SHALL run on provider error/timeout, rejection and rendering/navigation errors. A failed clear SHALL prevent subsequent navigation and be reported. Source: src/workflow/indicator.rs; src/workflow/mod.rs; src/workflow/orchestrator.rs.

#### Scenario: Successful navigation and output
- **WHEN** an accepted question moves from source to successor and writes an answer
- **THEN** the source has no temporary mark before the swipe, model/classifier/header captures contain no temporary mark and successful completion leaves no temporary mark on the answer page.

#### Scenario: Rejection or provider failure
- **WHEN** no readable question is found, transcription disagrees, or a model request fails or times out
- **THEN** the temporary mark is cleared before failure display or error propagation.

#### Scenario: Cleanup input failure
- **WHEN** clearing an owned mark fails
- **THEN** the workflow reports the error and does not proceed to navigation or write the model answer.
- **AND** it does not claim that the native mark was removed.

#### Scenario: Uncertain page after cleanup or typing failure
- **WHEN** cleanup fails or partial typing may have changed the corner
- **THEN** failed cleanup prevents further iterations on the same orchestrator, and status eligibility after typing is unknown until another clean capture.

#### Scenario: Invalid successor recovery takes priority
- **WHEN** the clean successor capture classifies the page as invalid
- **THEN** Reader attempts the verified return and source failure display without first drawing an activity triangle on the invalid successor.

#### Scenario: Long-running retracing
- **WHEN** a pending request repeats the same paths many times
- **THEN** cleanup erases each unique owned path with bounded work rather than replaying every tick.
