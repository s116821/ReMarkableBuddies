## Context

REM-11 originally requested Q @ coordinates. Its complete accessible comment history, reread with REM-31 and REM-5, supersedes that shape: December 9, 2025 at 11:06:10 UTC limits association to the initial question (55ffdaa9), 11:07:45 adds highlights (fedb3b45), and 21:26:48 requests recognizable start AND end delimiters (71501e9a). REM-5 at21:31:38 (d2fe4a27) explains their future scrolled-page recognition role. REM-31 has no comments at planning. These public artifacts reproduce the relevant requirements without requiring private-board access.

Current composition emits a coordinate-bearing Q line and --- footer. History counts all emitted ASCII bytes/newlines, owns only the exact appended block, and refuses uncertain native scene ownership. Historic native captures remain evidence of that older format.

## Goals / Non-Goals

**Goals:** exact requested opening/closing strings, matching normalized coordinates, plain Q inside, whole-block ownership and header/prior-answer preservation, demonstrated on simulator and authorized idle RM2.

**Non-Goals:** follow-up extraction or model page classification (REM-5), About fields (REM-17), triangle indicators (REM-32), rewriting old notes, weakening ownership or enlarging the existing 2000-character/32-paragraph limits.

## Decisions

### Pure complete-block composition

Keep formatting in Workflow::compose_qa. Format the same NormalizedCenter once into both boundaries. Emit `<Start of Q-A block for Q @ (x, y)>`, newline, `Q: question`, two newlines, `A: answer`, newline, `<End of Q-A block for Q @ (x, y)>`, final newline. A single-line answer therefore owns five newlines; multiline answers add their own. Remove the obsolete --- footer for newly produced blocks. Retaining both footer forms would add noise and does not satisfy a separate requirement.

### Existing content remains exact

Append new blocks without migrating existing plain-Q or Q-@ blocks. The current page header recognizer remains unchanged. Historical blocks continue to be user text; future follow-up extraction must explicitly handle legacy text conservatively and cannot assume it has new boundaries. Do not introduce an unneeded parser or claim scrolled-page recognition now exists.

### Native ownership covers both ends

History takes the complete composed string, so all characters, delimiters and final newline contribute to its existing bounds and paragraph deletion. Add meaningful regression assertions for complete removal/restoration, multiple-block preservation and exact size/paragraph limits. Preserve historical captured fixtures unchanged and add new native before/applied/deleted/restored evidence when practical. No partial footer deletion or relaxed scene checks is permitted.

### Verification

Exact pure formatting tests include normalized endpoints and multiline content. Simulator cases cover outline/highlight sources, first/new block, existing old-format content, repeated undo/redo, partial render failures and oversized history refusal. Native tests use bounded input on disposable document clones, inspect actual parsed text and screenshots, exercise five-paragraph selection and full restore, preserve the cached header and original document. Capture the immediate pre-typing state when investigating the known append/history availability limitation; report unsupported history honestly instead of loosening guards.

## Risks / Trade-offs

- Longer boundary lines can visually wrap -> verify actual rendering and paragraph-based whole-block selection; visual wrapping is not a typed newline.
- Extra characters can exceed existing ownership bounds -> preserve bounded refusal and test exact boundaries.
- Native scene changes can invalidate history -> retain fail-closed behavior, gather precise before/after evidence and distinguish old limitations from regressions.
- Delimiters do not by themselves implement follow-ups -> publish current scope and future consumer contract explicitly.

## Migration Plan

Ship new composition through the ordinary reviewed release. Existing notes remain untouched. Restore the authorized test document, cache and service after validation. Rollback changes only subsequent composition; it does not rewrite already rendered notes.

## Open Questions

No user decision is pending. Native full-block undo/redo is a required empirical gate; any unsupported case must be diagnosed and tracked before claiming acceptance.
