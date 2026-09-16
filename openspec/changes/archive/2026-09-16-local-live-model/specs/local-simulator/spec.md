## MODIFIED Requirements

### Requirement: Shared workflow execution
The simulator SHALL implement the same device-facing interface as the real tablet and run the production Reader orchestrator, model content/parsing/verification, image classification and recovery policies. Deterministic scenarios SHALL use scripted LLMEngine replies with no credentials or network calls. Explicit live scenarios SHALL use the configured provider through the same orchestration and result path. Source: src/device/backend.rs; src/simulator; src/workflow/orchestrator.rs.

#### Scenario: Accepted question
- **WHEN** proposal and independent transcription agree and the successor is blank
- **THEN** the production workflow writes one header and the exact Q&A block to the successor while preserving the source.

#### Scenario: Declined or disagreeing question
- **WHEN** a proposal is NONE or independent reading disagrees
- **THEN** the source receives an X without successor navigation or answer text.

### Requirement: Inspectable results and meaningful assertions
Every executed scenario SHALL export page PNGs and a JSON report of exact rendered text, marks, active page, model calls, errors and ordered timestamped operations. Expected-result mismatches SHALL fail the command after output is saved. Optional text_contains assertions SHALL check stable substrings while exact text assertions remain supported. Reports SHALL label scripted-offline or live-provider execution. Source: src/simulator/mod.rs; tests/simulator.rs.

#### Scenario: Failed assertion
- **WHEN** expected text, page, operation counts or error behavior differs
- **THEN** the process exits unsuccessfully and the report identifies the mismatch.


