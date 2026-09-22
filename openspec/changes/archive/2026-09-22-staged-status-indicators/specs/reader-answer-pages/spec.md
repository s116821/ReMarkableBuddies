## MODIFIED Requirements

### Requirement: Failure display and loop errors

Every failure outcome SHALL obey guarded display policy: clear all temporary owned marks, then draw the constant X plus exactly one of six segments in a centered half-size box within the existing status region. The top edge SHALL mean unreadable/missing/ambiguous selection or malformed proposal; right edge transcription disagreement/invalid transcription; bottom edge provider unavailable/timeout/transport failure; left edge no successor movement; horizontal midpoint line unsuitable successor after confirmed recovery; vertical midpoint line device/render/recovery failure. Existing/unknown corner content SHALL suppress drawing and erasure. Cleanup failure SHALL stop further input. Failure marks SHALL persist, never enter the temporary ledger, and be attempted at most once per iteration. Render errors SHALL be logged and attempt the device code. Single-iteration provider/device errors SHALL propagate after guarded display; loop mode SHALL log them without typing arbitrary Error text into the document. Source: src/workflow/indicator.rs; src/workflow/mod.rs; src/workflow/orchestrator.rs.

#### Scenario: Proposal transport error
- **WHEN** a proposal request returns a provider error
- **THEN** temporary activity is cleared and the provider X/code is attempted safely before the error propagates; loop mode does not insert Error text.

#### Scenario: Failure after visible progress
- **WHEN** an eligible page has activity and the question is declined
- **THEN** all owned paths are erased before the constant X and selection-code top edge are drawn.

#### Scenario: Six distinct causes
- **WHEN** selection, transcription, provider, no-motion, invalid-successor or device failures occur on eligible pages
- **THEN** each uses its documented unique segment over the same X.

#### Scenario: Unconfirmed recovery
- **WHEN** invalid-successor recovery cannot confirm the saved source
- **THEN** the device/recovery code is attempted only if the current corner is known safe.

#### Scenario: Existing corner handwriting
- **WHEN** the corner is occupied before the iteration draws status marks
- **THEN** the failure mark is suppressed without erasing existing handwriting.
