## MODIFIED Requirements

### Requirement: Current-page answer proposal
Each iteration SHALL capture the current page and send an overview plus three detail strips with ANALYSIS_PROMPT. The prompt SHALL ask for one readable handwritten question associated with a closed outline or deliberate highlighted passage, distinguish handwriting from typeset text and user selection from printed shading, allow surrounding context/general knowledge, preserve paper-specific values, and request plain ASCII output. Highlights SHALL NOT require a surrounding closed outline. Missing selections or ambiguous question-to-selection association SHALL request abstention. SELECTION_CENTER SHALL replace OUTLINE_BOX and identify the approximate center of the selected outlined/highlighted content in the full-page overview's 768 by 1024 pixel coordinate frame, excluding question and detail-strip coordinates. Selection interpretation remains model-based rather than independently proven from pixels. Source: src/workflow/orchestrator.rs analyze_and_answer/ANALYSIS_PROMPT.

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
The parser SHALL require a first --- separator, an ANSWER: body prefix and nonempty, non-NONE question/answer values. It SHALL retain the question-box parsing gate and require exactly one SELECTION_CENTER field with two finite numbers inside the inclusive overview canvas. It SHALL normalize x by overview width and y by overview height, preserve the complete answer after the first separator, and not independently prove a selection exists from pixels. Source: src/workflow/orchestrator.rs parse_analysis_response/parse_bounding_box.

#### Scenario: Malformed response
- **WHEN** required fields are absent, empty or explicitly NONE
- **THEN** no answer is rendered.

#### Scenario: Question bounds gate
- **WHEN** a proposal lacks a parsed question box, has Y outside 0..1024 or nonpositive height
- **THEN** independent verification declines it.
- **AND** question X/width remain outside the existing transcription gate; selected-content center validation is independently required before output.

#### Scenario: Invalid selection center
- **WHEN** SELECTION_CENTER is missing, duplicated, malformed, non-finite or outside the overview canvas
- **THEN** the proposal is declined before verification/navigation with no guessed, clamped or untagged answer.

#### Scenario: Device-independent center
- **WHEN** the selected-content center is (384, 512) in the full-page overview
- **THEN** its normalized center is (0.5, 0.5) regardless of native screen resolution.