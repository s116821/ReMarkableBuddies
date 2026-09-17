## ADDED Requirements

### Requirement: Reproducible history sessions
The simulator SHALL drive the shared last-Q&A history state using declared undo/redo holds, departures/returns, new iterations and intervening edits. Reports SHALL expose transaction state, ordered events and exact page text. Faults SHALL exercise uncertain/partial mutation without silently claiming rollback. Source: src/simulator/scenario.rs; src/simulator/device.rs; tests/simulator.rs.

#### Scenario: Repeated toggle
- **WHEN** a scripted session accepts a Q&A and alternates valid undo/redo actions
- **THEN** only that block toggles, with no extra model calls and unchanged earlier page content.

#### Scenario: Invalidated session
- **WHEN** a page departure, new iteration or intervening edit invalidates the record
- **THEN** later history actions cannot revive it or mutate unrelated content.

#### Scenario: Partial operation
- **WHEN** a history mutation fault occurs
- **THEN** the report retains the actual failure state and history is discarded without an automatic retry.

#### Scenario: Persisted page lags visible typing
- **WHEN** the visible Q&A has completed but the persisted snapshot still contains the prior text
- **THEN** history remains unavailable until the complete expected content is observed; a timeout never arms stale history.

#### Scenario: Gesture cancellation and interrupted input
- **WHEN** contacts move, change identity, drop out, remain partly released too long, or input events are lost
- **THEN** the shared contact policy invalidates history without executing a mutation; valid stationary two-/four-contact holds execute at most once after full release.

#### Scenario: Native preservation evidence
- **WHEN** recorded native snapshots are replayed through the history policy
- **THEN** previous visible paragraphs, native ink and opaque records must be preserved, and restored Q&A text/styles must equal the applied state; reports distinguish this replay from live keyboard, physical gesture or vision validation.

#### Scenario: Stable metadata describes another view
- **WHEN** saved last-opened metadata refers to the wrong page, or the user opens the overview without changing the saved page ID
- **THEN** history cannot arm from the wrong observation, and the overview input invalidates existing ownership even when the saved ID stays unchanged.

#### Scenario: Reader release ordering
- **WHEN** a corner hold reaches two seconds while still touching the screen
- **THEN** the shared reducer waits for release before dispatching Reader, preventing the observed native menu from reopening after dismissal; canceled or moved holds cannot trigger Reader.

#### Scenario: Current identity absent after native restart
- **WHEN** the document is visible but native current-document identity is unavailable
- **THEN** the answer can render normally, but subsequent undo/redo does not gain ownership retroactively.
