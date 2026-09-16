# reader-analysis

## Purpose

Describe the implemented reader analysis contracts, initially baselined from v0.1.4. Known gaps are explicit and require a later change delta to alter.

## Requirements

### Requirement: Current-page answer proposal
Each iteration SHALL capture the current page and send an overview plus three detail strips with ANALYSIS_PROMPT. The prompt SHALL ask for one readable handwritten question associated with a closed outline or deliberate highlighted passage, distinguish handwriting from typeset text and user selection from printed shading, allow surrounding context/general knowledge, preserve paper-specific values, and request plain ASCII output. Highlights SHALL NOT require a surrounding closed outline. Missing selections or ambiguous question-to-selection association SHALL request abstention. OUTLINE_BOX SHALL remain the response field for the selected outlined/highlighted region's overview bounding box. Selection interpretation remains model-based rather than independently proven from pixels. Source: src/workflow/orchestrator.rs analyze_and_answer/ANALYSIS_PROMPT.

#### Scenario: No readable question
- **WHEN** the response begins with NONE ignoring case and outer whitespace
- **THEN** the proposal is declined and the workflow draws a failure X without navigating for an answer.

#### Scenario: Highlighted concept
- **WHEN** a readable handwritten question clearly refers to a deliberate highlighted passage without a closed outline
- **THEN** the prompt permits an answer about that selected concept using the same format and independent question verification as a circled selection.

#### Scenario: Closed-outline compatibility
- **WHEN** the question clearly refers to content within a closed outline
- **THEN** the existing selection remains accepted under the same question-verification and answer-placement rules.

#### Scenario: Absent or ambiguous selection
- **WHEN** the page has a question but no clear user highlight/closed outline, only printed gray shading or an X, or an unclear association among selected topics
- **THEN** the prompt requests NONE rather than inventing a selected concept.

#### Scenario: Highlight without readable question
- **WHEN** highlighted content has no question or essential handwritten words are illegible or ambiguous
- **THEN** the prompt requests NONE and does not substitute a passage summary for the missing question.

### Requirement: Structured proposal parsing
The parser SHALL require a first --- separator, an ANSWER: body prefix and nonempty, non-NONE question/answer values. It SHALL parse optional four-integer boxes, preserve the complete answer after the first separator, and not independently prove an outline exists from pixels. Source: src/workflow/orchestrator.rs parse_analysis_response/parse_bounding_box.

#### Scenario: Malformed response
- **WHEN** required fields are absent, empty or explicitly NONE
- **THEN** no answer is rendered.

#### Scenario: Question bounds gate
- **WHEN** a proposal lacks a parsed question box, has Y outside 0..1024 or nonpositive height
- **THEN** independent verification declines it.
- **AND** X/width and outline bounds are not additional validation gates in this baseline.

### Requirement: Independent transcription agreement
Before navigation or answer output, the system SHALL clear model content and request transcription from the same overview/detail images without the proposed question or answer. It SHALL require a nonempty TRANSCRIPTION: value that agrees after normalization; verification request errors SHALL decline the proposal. Source: src/workflow/orchestrator.rs verify_question/transcriptions_agree.

#### Scenario: Harmless variation
- **WHEN** case, question punctuation or spacing around operators differs
- **THEN** equivalent normalized text can agree.

#### Scenario: Meaningful disagreement
- **WHEN** operators, numeric separators, grouping or word boundaries differ, or transcription is NONE/missing
- **THEN** verification fails and an X is drawn without writing the proposed answer.
- **AND** agreement remains a comparison check rather than proof of semantic correctness.

### Requirement: Model request contract
The OpenAI implementation SHALL send one user message containing accumulated multimodal content to base_url/v1/chat/completions with bearer authentication and max_completion_tokens 4000. Images SHALL use high detail. It SHALL extract choices[0].message.content as text. Source: src/llm/openai.rs execute.

#### Scenario: Successful recognized question
- **WHEN** proposal and independent verification both run successfully
- **THEN** two separate model requests occur with no shared conversation history beyond separately supplied current-page images.

#### Scenario: API failure boundary
- **WHEN** the proposal HTTP request fails
- **THEN** the error propagates to the caller rather than becoming a normal NONE response.
- **AND** malformed, unreadable or missing-content responses return errors without panicking; automatic retries are not added.
