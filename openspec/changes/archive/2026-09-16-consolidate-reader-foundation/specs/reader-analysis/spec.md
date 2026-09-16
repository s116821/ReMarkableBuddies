## MODIFIED Requirements

### Requirement: Model request contract
The OpenAI implementation SHALL send one user message containing accumulated multimodal content to base_url/v1/chat/completions with bearer authentication and max_completion_tokens 4000. Images SHALL use high detail. It SHALL extract choices[0].message.content as text. Source: src/llm/openai.rs execute.

#### Scenario: Successful recognized question
- **WHEN** proposal and independent verification both run successfully
- **THEN** two separate model requests occur with no shared conversation history beyond separately supplied current-page images.

#### Scenario: API failure boundary
- **WHEN** the proposal HTTP request fails
- **THEN** the error propagates to the caller rather than becoming a normal NONE response.
- **AND** malformed, unreadable or missing-content responses return errors without panicking; automatic retries are not added.
