---
name: reader-simulator-testing
description: Develop or debug Reader Buddy locally using deterministic simulator fixtures and authorized live-model checks. Use for scenario selection, regression coverage and interpretation of local test evidence; native device operations use the separate unattended tablet skill.
---

# Reader simulator development testing

Use this as an agent skill or manual checklist from the repository root. Offline
work requires Rust and repository files, not SSH, API keys, Codex or a private
board. Follow the public task/PR discussion and canonical specs; maintainers also
read complete timestamped private comments when already accessible and publish
relevant decisions. State genuinely missing context once and continue independent
work; ask only when a missing decision prevents correct work. A missing optional
integration is not a reason to request credentials or installation repeatedly.

## Choose evidence for the change

- **Scripted offline:** exercise the production workflow with declared replies,
  faults and contacts. This proves the asserted policy, not handwriting recognition.
- **Live provider:** for changes to prompts, image/model interpretation or model
  integration, run representative authorized real-model checks before merge.
  Existing authorization remains effective; application API usage is distinct
  from assistant/Codex capacity or credit restrictions. Contributors without live
  access can submit local work and identify the gate for a maintainer to complete.
- **Native device:** physical input, framebuffer reliability, actual typography,
  UI restoration and native persistence require appropriate maintainer hardware
  evidence. Use [unattended tablet testing](../reader-buddy-testing/SKILL.md) only
  with an authorized idle development tablet. Simulator success grants no tablet
  permission and cannot establish Writer/combined support before it is implemented.

Documentation-only changes need working examples and routing review, not a new
paid model run or native mutation solely to restate existing evidence.

## Select and exercise fixtures

Read the [scenario guide](../../../docs/simulator.md) for the schema and fidelity
limits, then choose maintained scenarios matching the changed behavior. For example:

```sh
cargo run -- --simulate docs/simulator/scenarios/blank-answer.json
cargo run -- --simulate docs/simulator/scenarios/disagreement.json
cargo test --test simulator
```

These examples remain offline even if local credentials exist. Start from a
maintained JSON scenario instead of adding a one-off production flag. Asset/output
paths resolve relative to the scenario. Use bounded iterations and reachable faults;
unknown fields and unused replies/faults are errors.

Assert exact scripted text and delimiters, expected page/history/failure code,
unchanged prior content and forbidden navigation/typing/model calls. Include a
relevant negative case: absent/illegible question, disagreement, occupied successor,
failed return, partial output, lost history ownership or failed status restoration.
Use the shared workflow and operation model rather than duplicating production logic.

Inspect `report.json` and page PNGs in the scenario's output directory. Check
`model_mode`, `assertion_failures`, exact page text, errors and ordered operations.
An expected refusal can pass assertions; exit zero alone does not mean an answer
was produced. The PNG uses approximate uppercase bitmap lettering (some glyphs
are placeholders); exact report text does not prove native keyboard typography.
Virtual milliseconds are requested/modelled time, not measured user latency.

## Live checks and findings

Use [local live-model setup](../../../docs/local-development.md) for ignored `.env`
or process environment configuration and explicit live scenarios. Never print keys
or put them in commands, scenarios, screenshots or commits. Check ignore/access
permissions and credential presence without reading values into evidence. Missing
credentials or repeated unchanged connectivity failure ends that live attempt;
retain independent offline results and identify the remaining maintainer gate.
Do not silently substitute scripted replies for an unperformed live test.

For model-facing changes, select representative actual inputs from the
[fixture guide](../../../docs/validation/README.md): connected cursive/shorthand,
varying spacing/slant, and applicable absent/ambiguous questions. Inspect the
annotated input, independent reading, selected region and resulting explanation.
Check paper-specific values, units and operators; agreement alone is not proof of
correct reading. Bound calls/timeouts with the existing live scenario fields.
Live assertions should cover stable required facts and refusal behavior without
requiring one arbitrary wording. No blanket provider permission is implied.

Preserve failing fixtures, logs and output alongside subsequent fixes. Do not replace
a misread with easier print and relabel it a pass, or claim synthetic strokes are
human handwriting. Keep raw material local unless intended for publication; model
outputs and logs can contain document content even when credentials are omitted.

When native findings expose missing modeled behavior, extend the shared simulator,
faults, fixtures or assertions in the same implementation PR where feasible. State
unmodeled UI/persistence/physical limits explicitly and retain required native gates.
Record source revision, actual expected/observed result and evidence category in
PR comments; distinguish current checks from older proof. Follow the
[OpenSpec workflow](../../../openspec/README.md) to sync and archive in the same PR.
