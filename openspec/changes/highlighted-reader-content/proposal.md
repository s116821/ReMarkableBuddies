## Why

REM-16 extends Reader Buddy to questions about highlighted passages. The current analysis prompt requires a closed outline even when a deliberate highlight clearly selects the concept.

## What Changes

- Accept either a closed outline or deliberate highlight around the relevant technical concept.
- Preserve readable handwritten questions, conservative abstention, independent transcription and paper-specific numeric fidelity.
- Keep the existing response format; OUTLINE_BOX denotes the selected outlined/highlighted region for compatibility.
- Preserve highlighted text contrast in modern RM2 screenshots using neutral-preserving luminance instead of only the blue channel.
- Add representative native highlighted input and local deterministic/live regression evidence, with final hardware checks where appropriate.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- reader-analysis: extend current-page proposal selection criteria to highlights without weakening question recognition or ambiguity rejection.
- tablet-io: preserve contrast in colored highlighter pixels while retaining neutral grayscale values.

## Impact

Analysis prompt, modern RM2 grayscale conversion, explanatory comments/docs, maintained fixtures/tests, canonical analysis and tablet-I/O specifications. No navigation, rendering, model-provider or native insertion changes.
