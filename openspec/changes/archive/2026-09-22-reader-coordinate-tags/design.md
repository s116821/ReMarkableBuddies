## Context

Reader already receives a full-page 768 by 1024 overview and three overlapping detail strips. Both hardware branches normalize the overview to this frame. The model currently returns a question box, used by independent transcription validation, and an unused selected-content outline box. Q&A composition currently omits location.

## Goals / Non-Goals

Goals: attach a compact device-independent location to every accepted Q&A; identify the selected content rather than the handwritten question; preserve circle/highlight handling, independent reading and undo/redo boundaries.
Non-goals: precise geometry, pixel-based selection verification, links/navigation from tags, extra model calls, legacy answer rewriting, native page insertion or other roadmap features.

## Decisions

- Replace only OUTLINE_BOX with SELECTION_CENTER: x,y in the existing full-page overview pixel frame (768 by 1024). Keep QUESTION_BOX because it supports the existing transcription gate. Explicitly tell the model that detail-strip coordinates and question location are not the selected-content center. Asking for a center directly matches REM-11; deriving it from an unused approximate bounding box would retain unnecessary output.
- Normalize in Rust using a reusable validated center type: x / width and y / height. The production width/height are the shared overview constants, so the same physical proportion yields the same tag on RM2 and Paper Pro. Reject non-finite numbers, invalid/zero dimensions, malformed field counts, missing/duplicate center fields and coordinates outside the inclusive canvas bounds. Do not clamp or silently use (0.5, 0.5).
- Format each normalized axis to at most two decimal places, trimming redundant zeroes and decimal points; normalize negative zero to zero. This exposes approximate rather than false high precision. Render Q @ (x, y): question, followed by the existing blank line, answer and separator.
- Require a valid center before independent verification/navigation. Retain all existing question/answer gates. Failed location parsing follows the ordinary declined-proposal path; it never emits a guessed or untagged answer.
- Pass the typed validated center into pure composition. Register the entire tagged Q&A as the existing history transaction, preserving earlier untagged answers and the header. No migration of native text files.
- Update maintained scripted responses and expected text across simulator/live HTTP tests. Add malformed/out-of-bounds/missing-coordinate rejection, circle/highlight center distinction and tagged-history regression. Simulator fixtures prove routing and composition, not actual visual localization. Native/live acceptance must compare tags with the selected region in screenshots.

## Risks / Trade-offs

- Model chooses question coordinates or strip coordinates -> explicit prompt, targeted fixtures and visual native checks. In-bounds validation cannot establish semantic localization.
- Extra required field causes conservative declines -> retain failures and test representative circle/highlight examples without loosening recognition agreement.
- Formatting changes snapshots/history lengths -> update exact expectations, retain newline count and caps, test undo/redo on tagged blocks and earlier untagged text.
- Other hardware remains untested -> normalized math and ARM build are distinguished from Paper Pro hardware acceptance.

## Migration Plan

Complete specs/tasks first; implement and verify shared behavior, then test an isolated authorized native clone. Preserve the installed REM-15 rollback and original document/header/tool/service. Sync canonical specs and archive in this implementation PR; use independent final-head review and green CI/Bugbot before normal merge. Respect the 10% Codex reserve/no-credit constraint throughout.

## Open Questions

No blocking product questions. Hardware/model localization quality is an acceptance gate, not assumed from parser tests.
