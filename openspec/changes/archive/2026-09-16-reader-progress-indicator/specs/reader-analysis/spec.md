## MODIFIED Requirements

### Requirement: Model request contract

The OpenAI implementation SHALL send one user message containing accumulated multimodal content to base_url/v1/chat/completions with bearer authentication and max_completion_tokens 4000. Images SHALL use high detail. It SHALL extract choices[0].message.content as text. Production requests SHALL have a 90-second global timeout; explicit development timeouts SHALL remain honored. During progress-aware calls only HTTP work SHALL run off the workflow thread; progress callbacks SHALL run on the caller thread and stop when complete or failed. Source: src/llm/mod.rs; src/llm/openai.rs execute/execute_with_progress.

#### Scenario: Successful recognized question
- **WHEN** proposal and independent verification both run successfully
- **THEN** two separate model requests occur with no shared conversation history beyond separately supplied current-page images.

#### Scenario: API failure boundary
- **WHEN** the proposal HTTP request fails
- **THEN** the error propagates to the caller rather than becoming a normal NONE response.
- **AND** malformed, unreadable or missing-content responses return errors without panicking; automatic retries are not added.

#### Scenario: Timeout and callback failure
- **WHEN** a provider exceeds its configured timeout or a progress callback fails
- **THEN** no response is rendered as an answer, further progress callbacks stop, and the bounded worker completes without device access before the call returns an error.
### Requirement: Independent transcription agreement
Before navigation or answer output, the system SHALL clear model content and request transcription from the same overview/detail images without the proposed question or answer. It SHALL require a nonempty TRANSCRIPTION: value that agrees after normalization; verification provider errors SHALL decline the proposal, while device progress or cleanup errors SHALL propagate and prevent answer output. Source: src/workflow/orchestrator.rs verify_question/transcriptions_agree.

#### Scenario: Harmless variation
- **WHEN** case, question punctuation or spacing around operators differs
- **THEN** equivalent normalized text can agree.

#### Scenario: Meaningful disagreement
- **WHEN** operators, numeric separators, grouping or word boundaries differ, or transcription is NONE/missing
- **THEN** verification fails and an X is drawn without writing the proposed answer.
- **AND** agreement remains a comparison check rather than proof of semantic correctness.

#### Scenario: Device failure during verification
- **WHEN** an indicator callback fails during the independent verification wait
- **THEN** the iteration reports that error after cleanup rather than treating it as a successful question decline.
