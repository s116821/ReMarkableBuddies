## Context

Reader currently requires a closed outline in ANALYSIS_PROMPT. The model returns an optional OUTLINE_BOX, while independent verification checks the handwritten question. REM-22/REM-28 now provide offline and live local regression paths.

## Goals / Non-Goals

Goals: answer a readable handwritten question associated with a deliberate highlight or closed outline; preserve conservative question recognition and current answer workflow.
Non-goals: selection tagging/linking (REM-11), native insertion, new provider, follow-up history, rendering/navigation changes, arbitrary underlining interpretation or guarantees for every handwriting style.

## Decisions

- Extend the shared ANALYSIS_PROMPT used by production and diagnostics. Explain that a deliberate highlighted passage can select a concept without a surrounding closed outline, including grayscale tablet highlights. Retain the readable-question and clear-association conditions; printed gray figures/shading and an X are not selections.
- Keep OUTLINE_BOX as the wire field and define it as the bounding box of the selected outlined/highlighted region. Renaming response fields now would create unnecessary compatibility work before REM-11. Do not add a new image-classification algorithm: selection interpretation remains the model's task and its limits are explicit.
- Preserve independent transcription unchanged. Selection expansion must not turn printed prose into questions or permit guessing missing/ambiguous question words.
- Maintain captured native highlighter fixtures, deterministic routing scenarios and separate explicit live cases. Scripted assertions establish workflow behavior; real-provider output is required for visual interpretation evidence. Include existing circled input, no selection, absent question and illegible question.
- Final dev-tablet acceptance uses the testing skill, a disposable technical-paper copy and the final executable. Check quantitative shorthand on blank successor, a different cursive question appending, occupied recovery, held/short triggers and rejection cases. Save failures, preserve original document/cache and restore service.

## Risks / Trade-offs

- Gray printing may resemble highlight -> prompt distinguishes intentional markings and ambiguous association declines; use negative live fixtures.
- Prompt broadening may reduce question conservatism -> retain independent reread and absent/illegible cases.
- Vision is nondeterministic -> inspect actual question/answer and scientific values; do not claim scripted replies prove perception.

## Migration Plan

Existing closed-outline inputs and response format remain supported. Stage final build with rollback for bounded dev-tablet tests; restore normal service and original document/cache. Sync canonical analysis spec and archive with implementation/evidence in one PR.

## Open Questions

None.

## Capture prerequisite discovered during native fixture preparation

Native yellow highlights store colored BGRA (observed R/G254,B125) on RM2 despite its monochrome display. Blue-only encoding darkens the highlight and reduces printed-text contrast. Convert modern RM2 pixels with (77R+150G+29B+128)>>8: weights sum to256, preserving all neutral gray values exactly and making yellow lighter. Keep allocation discovery, legacy and Paper Pro branches unchanged. Add neutral/orientation and colored-highlight contrast tests; retrieve and visually inspect final native capture before claiming improvement. This is a scoped prerequisite to highlight interpretation, not a general capture redesign.

