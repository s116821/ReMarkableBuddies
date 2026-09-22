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


#### Scenario: Ready state on a valid successor
- **WHEN** navigation reaches an eligible valid answer page
- **THEN** the current AnswerReady triangle completes all three edges at the same cadence before cleanup and body-mode output; preceding source-page stages are not restarted.


#### Scenario: Accepted input without visible erasure
- **WHEN** native erase input returns success but a fresh screenshot still contains status marks or changed corner content
- **THEN** cleanup fails and further input stops rather than declaring the region clean from command success alone.

## ADDED Requirements

### Requirement: Scoped readable native status style

Reader SHALL use a verified narrow black status style independently of the user's selected broad pen, without permanently changing the active slot or stored tool/color/width preferences. A bounded style lease SHALL snapshot document/page and exact preferences before mutation, verify supported toolbar/menu state, and restore them after temporary cleanup or persistent failure drawing before further capture/navigation/typing. Repeated333ms strokes SHALL perform no toolbar toggles. Hidden toolbar, already-open menu, active non-pen tool, unsupported layout or ambiguous identity SHALL suppress status without mutation. Partial acquisition SHALL attempt bounded verified rollback; failed restoration SHALL halt further input. A recovery record SHALL retain original preferences across a crash without pretending finally ran or automatically overwriting later user choices. Source: planned src/device/status_style.rs; src/device/backend.rs; src/workflow/mod.rs.

#### Scenario: Highlighter selected
- **WHEN** a verified supported page has Highlighter selected and a clear corner
- **THEN** triangles, auxiliary circle and failure-code segments use narrow black strokes, and exact original tool/preferences return before continuation.

#### Scenario: Partial acquisition or error mark failure
- **WHEN** a status-style action or persistent marker fails after possibly mutating state
- **THEN** bounded rollback verifies restored preferences or stops further document input with its recovery record retained, without recursively drawing another error.

#### Scenario: Unsupported or already-open controls
- **WHEN** the toolbar/menu or page identity cannot be safely matched before acquisition
- **THEN** Reader does not toggle controls, write preferences or draw status in the selected user style.

#### Scenario: Native history and status cadence
- **WHEN** undo/redo owns the previous Q&A or a pending stage repeats
- **THEN** no toolbar manipulation occurs during history, and a pending stage reuses its lease rather than selecting a tool every333ms.

#### Scenario: Interrupted process
- **WHEN** a process terminates before restoration
- **THEN** the recovery record remains and later runs refuse automatic status mutation until deliberate recovery, without claiming the user's preferences were restored.
