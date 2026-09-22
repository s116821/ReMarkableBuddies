## Why

REM-11 lets readers connect an answer with the circled or highlighted source region. The current Q&A omits location even though analysis already identifies selection geometry.

## What Changes

- Ask analysis for the approximate center of the selected outlined or highlighted content in the full-page overview coordinate space, retaining the question box needed for independent transcription checks.
- Validate the center and normalize each axis by the corresponding overview dimension before rendering `Q @ (0.5, 0.5): question`.
- Reject missing, malformed or out-of-range location instead of inventing a tag; preserve existing question agreement and answer-page safety.
- Update shared composition, history/simulator regressions, live fixtures and documentation. Verify native circled and highlighted cases with source-region location evidence.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- reader-analysis: validated selected-content center in model responses, distinct from handwritten question geometry.
- reader-answer-pages: normalized coordinate tag in each Q&A question line.
- local-simulator: coordinate parsing, rendering, decline and history preservation coverage through the shared workflow.

## Impact

Analysis prompt/parser, coordinate value type and pure formatting, Q&A composition, scripted/live fixtures, shared history tests and native acceptance. No new provider, model request, tablet firmware change, page insertion or persistent memory.
