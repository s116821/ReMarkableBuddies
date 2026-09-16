## MODIFIED Requirements

### Requirement: Failure display and loop errors

Expected declines SHALL attempt an X within the same 50 by 50 virtual-pixel bottom-right status region used by the activity circle, after clearing any owned circle. If preexisting content makes status drawing unsafe, the failure mark SHALL be suppressed without erasing that content. Render-answer errors SHALL be logged and attempt an X after cleanup. Unhandled iteration errors in loop mode SHALL attempt body-mode Error: text on the current page after cleanup before continuing; single-iteration errors SHALL propagate. Source: src/workflow/mod.rs draw_failure_x; src/workflow/indicator.rs; src/workflow/orchestrator.rs run_iteration/run_loop.

#### Scenario: Proposal transport error in loop mode
- **WHEN** a proposal request returns an error
- **THEN** the loop attempts error text on the currently active page after indicator cleanup, rather than guaranteeing an X-only failure.

#### Scenario: Failure after visible progress
- **WHEN** an eligible page has a circle and the question is declined
- **THEN** the circle is erased before the two failure-X diagonals are drawn within the 50 by 50 region.

#### Scenario: Existing corner handwriting
- **WHEN** the corner is occupied before the iteration draws status marks
- **THEN** the failure mark is suppressed rather than erasing preexisting handwriting.
