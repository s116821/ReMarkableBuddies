## ADDED Requirements

### Requirement: Development fixture and acceptance practice
The simulator testing skill SHALL choose maintained fixtures by changed behavior, assert exact text/forbidden operations and preserve negative cases and failed artifacts. Applicable model-facing checks SHALL include representative connected cursive/shorthand and ambiguous or absent questions, with scientific values and actual input/output inspected. Native findings SHALL extend the shared model, faults, fixtures or assertions in the same implementation PR where representable; remaining fidelity gaps SHALL be explicit. Source: REM33; .agents/skills/reader-simulator-testing/SKILL.md; docs/simulator.md; docs/validation/README.md.

#### Scenario: Expected rejection
- **WHEN** a no-question, transcription-disagreement or invalid-successor fixture runs
- **THEN** its assertions check the intended refusal, no forbidden navigation/output and preserved unrelated content rather than equating successful exit with a successful answer.

#### Scenario: Native-only behavior discovered
- **WHEN** device observation reveals behavior the current simulator omits
- **THEN** the implementation PR adds meaningful model/regression coverage when possible and retains native evidence or an explicit unsupported limit without claiming emulation of unmodeled typography, persistence or physical input.
