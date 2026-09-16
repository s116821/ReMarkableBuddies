## MODIFIED Requirements

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
