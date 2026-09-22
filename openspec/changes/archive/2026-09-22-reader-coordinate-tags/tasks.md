## 1. Shared coordinate contract

- [x] 1.1 Add validated pixel-to-normalized selected-center representation and compact formatting with finite/bounds/resolution tests.
- [x] 1.2 Update prompt and parsing for exactly one valid SELECTION_CENTER while retaining independent question-box/transcription gates.
- [x] 1.3 Render the tag in shared Q&A composition and preserve the entire tagged history boundary.

## 2. Regressions and acceptance

- [x] 2.1 Update maintained scripted/live HTTP fixtures and exact expectations; add invalid-center, circle/highlight association and tagged-history regressions.
- [x] 2.2 Pass relevant host tests, strict host/ARM lint, formatting and both ARM release builds.
- [x] 2.3 Verify live/native circled and highlighted center tags, rendered text and history on an isolated technical-paper clone; restore original document/header/tool/service and retain attributed evidence.

## 3. Delivery

- [x] 3.1 Update documentation, verify requirement/design coverage, synchronize canonical specs and archive this change in the implementation PR.
- [x] 3.2 Install and verify the tested runtime; preserve rollback and restore the original document/header/tool/service.

Fresh independent final-head review, visible PR evidence, green CI/Bugbot and normal merge remain delivery gates, checked against the final committed candidate.
