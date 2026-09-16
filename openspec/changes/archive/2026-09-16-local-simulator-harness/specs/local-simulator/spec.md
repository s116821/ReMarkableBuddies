## ADDED Requirements

### Requirement: Shared workflow execution
The simulator SHALL implement the same device-facing interface as the real tablet and run the production Reader orchestrator, model content/parsing/verification, image classification and recovery policies. Deterministic scenarios SHALL use scripted LLMEngine replies with no credentials or network calls. Source: src/device/backend.rs; src/simulator; src/workflow/orchestrator.rs.

#### Scenario: Accepted question
- **WHEN** proposal and independent transcription agree and the successor is blank
- **THEN** the production workflow writes one header and the exact Q&A block to the successor while preserving the source.

#### Scenario: Declined or disagreeing question
- **WHEN** a proposal is NONE or independent reading disagrees
- **THEN** the source receives an X without successor navigation or answer text.

### Requirement: Structured reproducible scenarios
Scenarios SHALL specify initial page snapshots, active page, bounded iterations, gesture frames, scripted model replies, operation-indexed faults and expected results. Unknown fields, unsupported modes and invalid page/fault/gesture values SHALL fail before execution. Relative asset/output paths SHALL resolve from the scenario location. Source: src/simulator/scenario.rs; src/simulator/mod.rs.

#### Scenario: Bounded gesture input
- **WHEN** a short contact precedes a stationary two-second corner hold
- **THEN** the shared timer ignores the short contact and triggers on the valid hold without movement.

#### Scenario: Exhausted gestures
- **WHEN** a scenario has no valid remaining trigger
- **THEN** execution records a bounded error instead of waiting forever.

### Requirement: Page and fault model
The simulator SHALL retain ordered pages and page-local text/marks, model end boundaries, cached header recognition and requested delays, and inject capture, input, rendering, no-motion and stale-capture conditions at declared operation calls. Shared recovery SHALL never exceed one reverse swipe. Source: src/simulator/device.rs; src/workflow/navigation.rs.

#### Scenario: Existing answers
- **WHEN** a later iteration reaches a cached-header successor
- **THEN** it appends a Q&A block without duplicating the header or erasing prior answers.

#### Scenario: Failed return
- **WHEN** an occupied successor rejects output and its return swipe makes no movement
- **THEN** the X appears on the successor, no answer is written and no second return occurs.

#### Scenario: End page
- **WHEN** no successor exists
- **THEN** the workflow stays on the source and draws an X without reverse navigation or insertion.

### Requirement: Inspectable results and meaningful assertions
Every executed scenario SHALL export page PNGs and a JSON report of exact rendered text, marks, active page, model calls, errors and ordered timestamped operations. Expected-result mismatches SHALL fail the command after output is saved. Source: src/simulator/mod.rs; tests/simulator.rs.

#### Scenario: Failed assertion
- **WHEN** expected text, page, operation counts or error behavior differs
- **THEN** the process exits unsuccessfully and the report identifies the mismatch.

### Requirement: Honest fidelity and extension boundaries
Documentation SHALL attribute modeled dimensions, holds, delays, masks and recovery to observed RM2 behavior and distinguish scripted replies/raster output from hardware and live-model validation. Writer/combined execution SHALL report unsupported until production Writer is implemented; REM-23/REM-17 SHALL extend coverage and REM-25 SHALL extend insertion. Source: docs/simulator.md; src/simulator/scenario.rs.

#### Scenario: Unsupported product
- **WHEN** a scenario requests Writer or combined mode
- **THEN** validation fails explicitly without simulated claims of those workflows.
