# Baseline review

Source baseline: main b3dac64, release v0.1.4. REM-27 is documentation-only.

| Capability | Requirements / scenarios | Implementation evidence |
| --- | --- | --- |
| platform-runtime | 4 / 8 | src/main.rs Args/main; src/llm/openai.rs new/from_env; deploy/reader-buddy.service |
| tablet-io | 4 / 8 | src/device/mod.rs detect; screenshot.rs layout/allocation/encoding/detail methods; touch.rs trigger/transforms; keyboard.rs body/key handling; pen.rs input |
| reader-analysis | 4 / 7 | orchestrator.rs proposal/parser/verification/normalization; openai.rs request/response |
| reader-answer-pages | 5 / 8 | orchestrator.rs render/recovery/loop; workflow/mod.rs masks/classification/cache/X; xochitl_integration.rs swipes |

Completeness: four capabilities cover all 17 requirements and 31 scenarios with
source evidence. Official core plus verify/sync skills and project rules are present.
Correctness: reviewed behavior against the referenced source, not old planning notes.
Coherence: no runtime code/configuration changes; the four canonical requirements
sections match the baseline deltas exactly after sync.

Important drift caught during review: hold threshold is two seconds (not the
README's three-second user instruction); input-png/save-screenshot are inert;
question bounds validate only Y/height after parsing; outline presence is prompted
rather than deterministically checked; invalid-page recovery still retries three
times; loop-level errors can type on the current page; progress helpers are unused.
These are intentionally recorded, not repaired under REM-27.

Validation: strict CLI validation passes for the change and all four canonical
specs. No new model calls or tablet actions were performed. Existing source tests
cover parsing/abstention, normalization, firmware layout and detail tiles; many
navigation/configuration/gesture scenarios currently have source-only coverage.
REM-13/REM-22 own additional regression infrastructure. PR10's bounded RM2 evidence
does not establish human-handwriting, reboot, orientation or Paper Pro acceptance.
These coverage warnings are baseline limitations, not missing REM-27 implementation.

No critical drift or intentional behavior change found. Review findings are
resolved by accurately documenting current limitations rather than making fixes.
