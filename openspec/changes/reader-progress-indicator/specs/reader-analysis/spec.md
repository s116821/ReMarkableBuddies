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
