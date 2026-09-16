## ADDED Requirements

### Requirement: History interaction events
While awaiting the next Reader action, tablet input SHALL recognize stationary four-contact holds of two seconds as undo and two-contact holds of two seconds as redo. Recognition SHALL use complete contact frames and stable tracking identities, emit once per hold, and require complete release before another action. The existing single-contact corner hold SHALL still initiate Reader. Departure or editing events SHALL invalidate history ownership without concurrent device input. Source: planned src/device/touch.rs event reducer and src/device/backend.rs idle interaction boundary.

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
