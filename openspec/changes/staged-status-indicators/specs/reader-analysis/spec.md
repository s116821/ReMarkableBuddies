## MODIFIED Requirements

### Requirement: Independent transcription agreement
Before navigation or answer output, the system SHALL clear model content and request transcription from the same overview/detail images without the proposed question or answer. It SHALL require a nonempty TRANSCRIPTION: value that agrees after normalization; verification provider errors SHALL decline the proposal using the provider failure code, distinct from the transcription-disagreement code, while device progress or cleanup errors SHALL propagate and prevent answer output. Source: src/workflow/orchestrator.rs verify_question/transcriptions_agree.

#### Scenario: Harmless variation
- **WHEN** case, question punctuation or spacing around operators differs
- **THEN** equivalent normalized text can agree.

#### Scenario: Meaningful disagreement
- **WHEN** operators, numeric separators, grouping or word boundaries differ, or transcription is NONE/missing
- **THEN** verification fails and the transcription X/code is drawn without writing the proposed answer.
- **AND** agreement remains a comparison check rather than proof of semantic correctness.

#### Scenario: Device failure during verification
- **WHEN** an indicator callback fails during the independent verification wait
- **THEN** the iteration reports that error after cleanup rather than treating it as a successful question decline.

#### Scenario: Unavailable verification
- **WHEN** the verification provider fails or times out
- **THEN** no answer is written and the provider X/code is attempted after cleanup, rather than claiming transcription disagreement.
