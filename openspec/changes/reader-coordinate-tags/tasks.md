## 1. Shared coordinate contract

- [ ] 1.1 Add validated pixel-to-normalized selected-center representation and compact formatting with finite/bounds/resolution tests.
- [ ] 1.2 Update prompt and parsing for exactly one valid SELECTION_CENTER while retaining independent question-box/transcription gates.
- [ ] 1.3 Render the tag in shared Q&A composition and preserve the entire tagged history boundary.

## 2. Regressions and acceptance

- [ ] 2.1 Update maintained scripted/live HTTP fixtures and exact expectations; add invalid-center, circle/highlight association and tagged-history regressions.
- [ ] 2.2 Pass relevant host tests, strict host/ARM lint, formatting and both ARM release builds.
- [ ] 2.3 Verify live/native circled and highlighted center tags, rendered text and history on an isolated technical-paper clone; restore original document/header/tool/service and retain attributed evidence.

## 3. Delivery

- [ ] 3.1 Update documentation, verify requirement/design coverage, synchronize canonical specs and archive this change in the implementation PR.
- [ ] 3.2 Obtain independent final-head review and green CI/Bugbot, publish concise PR with visible evidence, install/verify final build and merge normally.
