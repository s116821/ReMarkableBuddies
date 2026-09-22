## ADDED Requirements
### Requirement: Measured responsive Reader interaction
Reader SHALL measure and distinguish trigger qualification/release, first feedback, provider request/response, answer rendering and restored readiness. Local phase costs and provider time SHALL be separated. Acceptance SHALL use repeated native before/after runs with ordinary Fineliner and nondefault primary/secondary Highlighter, successful Q&A, failure and history paths, using reviewed explicit latency budgets. A one-second saving SHALL NOT close the issue while dominant avoidable multi-second stalls remain. Source: REM9 September22 clarification.

#### Scenario: Measured improvement
- **WHEN** before/after responsiveness is reported
- **THEN** individual repeated values, operation counts, median/max, source/build/tool state and provider spans are retained; fixed text/conditions distinguish local gains from shorter model answers.

#### Scenario: Guarded refusal
- **WHEN** a native status guard stops before output
- **THEN** the run is reported as failed availability, not successful Q&A or ready-state latency; exact internal frames are captured only with diagnostic opt-in and ownership guards remain enforced.

#### Scenario: Incomplete performance gate
- **WHEN** budgets, successful workflow or required preservation checks remain unmet
- **THEN** REM9 stays incomplete and REM35 integrated acceptance is not implied by microbenchmark or simulator results.

### Requirement: Deterministic completion sequencing
Reader operations SHALL prefer supported completion events and verified postconditions over assuming success after a fixed delay. When usable events are unavailable, operations SHALL use paced fresh-state polling with monotonic deadlines and cancellation. Observations SHALL be correlated to the active operation, page and session; device mutations SHALL remain serialized with existing ownership, restoration and recovery guarantees. Fixed waits SHALL be documented protocol, measured physical or observability exceptions, with scope and validation. Legitimate gesture timing, cadence, polling intervals, backoff and timeouts SHALL remain distinct from completion assumptions. Source: REM9 architecture steering September22.

#### Scenario: Supported completion event
- **WHEN** a completion notification arrives for an active operation
- **THEN** only a fresh correlated verified postcondition advances the operation; the notification alone does not prove readiness.

#### Scenario: Missing or unusable signal
- **WHEN** supported notifications are unavailable or lost
- **THEN** bounded fresh-state polling returns on the verified predicate or fails/cancels at its bound; elapsed time alone never means success.

#### Scenario: Stale or repeated signals
- **WHEN** stale, duplicate, out-of-order or wrong-operation/page/session events arrive
- **THEN** they cannot repeat a mutation or complete another operation, and cancellation prevents late observations reviving cancelled work.
