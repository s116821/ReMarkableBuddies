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
