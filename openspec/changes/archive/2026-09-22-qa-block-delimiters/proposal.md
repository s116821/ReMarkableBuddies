## Why

The delivered coordinate-only Q prefix missed a later request for recognizable start and end delimiters. Correcting it preserves the user's intended association and supplies the boundaries needed by future scrolled-page recognition and follow-ups.

## What Changes

- Render matching `<Start of Q-A block for Q @ (x, y)>` and `<End of Q-A block for Q @ (x, y)>` around each new block, with ordinary Q: and A: content inside.
- Keep normalized approximate circle/highlight coordinates associated once with the initial question; do not rewrite historical answers.
- Include both delimiters in the bounded history transaction and verify complete undo/redo while preserving header, previous blocks and unrelated ink.
- Preserve historical native fixtures and add current-format simulator and native evidence.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `reader-answer-pages`: matching coordinate-bearing boundaries and historical-format preservation.
- `reader-qa-history`: explicit ownership and bounds include both delimiters.

## Impact

Pure Q&A composition, history character/paragraph accounting and regression fixtures; rendering uses the existing native keyboard path. No provider, dependency or credential changes. Follow-up extraction/inference remains REM-5; the About field remains REM-17. This is corrective REM-31 scope from the missed REM-11 comment chronology.
