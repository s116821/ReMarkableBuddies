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

Reader SHALL use a verified narrow black status style independently of the user's selected broad pen, without permanently changing the active slot or current tool/color/width preferences. The supported visible UI SHALL be authoritative for actual active slot, primary tool grid and Fineliner color/width; persisted tool preferences SHALL be advisory because they can be stale. A bounded staged lease SHALL durably record each rollback target before the corresponding mutation, restore from captured UI values before temporary cleanup or after persistent failure drawing, and verify actual controls before further capture/navigation/typing. If probing fails before color/width changes, rollback SHALL restore the original tool/slot without modifying those unmodified dimensions. Repeated333ms strokes SHALL perform no toolbar toggles. Hidden toolbar, already-open menu, active non-pen tool, unsupported layout or ambiguous identity SHALL suppress status without mutation; a safely rolled-back probe failure SHALL suppress status. Failed restoration SHALL halt further input. A bounded, versioned, exclusive owner-private recovery journal SHALL describe probe phase, captured values and may-have-mutated dimensions across a crash; later runs SHALL refuse automatic mutation rather than applying stale settings. Source: src/device/status_style.rs; src/device/backend.rs; src/workflow/mod.rs.

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


#### Scenario: Persisted preferences lag the UI
- **WHEN** the visible original slot/tool or Fineliner Red/Thick settings differ from the document file's old Black/Medium values
- **THEN** Reader captures and restores the visible values, treats the file as advisory, and never claims current preference restoration from file equality alone.

#### Scenario: Failure while inspecting Fineliner
- **WHEN** selecting Fineliner for inspection fails or its style cannot be read before color/width input
- **THEN** rollback restores the original primary grid/active slot without changing Fineliner color or width, or stops with its phase-specific recovery evidence if safe UI restoration cannot be verified.

#### Scenario: Restore before eraser redraw
- **WHEN** temporary activity paths need cleanup
- **THEN** Reader verifies and restores actual original tools with its strict page guard before eraser input, durably records pending cleanup and retains the recovery journal.
- **AND** after erasure it verifies fresh owner/session identity, original closed pen controls, the clean corner and the unchanged invariant page region outside the documented lower-right redraw region before removing the journal.
- **AND** failed restoration causes no erasure, while failed erasure or final verification retains recovery evidence and stops all further input without retry.

Before mutation, Reader SHALL require a completely blank non-toolbar canvas or fixed distributed landmarks: three 256x256 patches at (96,128), (416,128) and (96,576), each with at least six qualifying 64x64 cells spanning three rows and three columns; each cell requires at least16 pixels below200 and128 pixels at or above248. Sparse or confined-content pages SHALL suppress status before any input. Blank pages SHALL retain strict full-canvas comparison without a redraw exception. For landmark-qualified pages only, the post-erase invariant image guard excludes x>=576,y>=800 and the left toolbar; it is viewport evidence, not a claim that every pixel in the native redraw region survived. Native sentinel-ink checks in that region SHALL remain a required acceptance gate. The pre-restoration full-page guard SHALL remain unchanged.
