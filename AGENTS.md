# Repository guidance

## Pull request presentation

- Begin the PR body exactly with `# Summary`, followed by concise bullets explaining
  the changes. Do not add testing plans, validation sections, or report boilerplate.
- Put actual test evidence in grouped PR comments: embed visible screenshots,
  explain each observed result, identify the tested build, and state material limits.
  Distinguish live model tests, offline replay, and final-build regression checks.
- Prefer durable GitHub attachments. If unavailable, retain only the minimal
  repository-hosted images needed for inline comments and pin their URLs to a commit.
  Verify rendering before removing redundant evidence from the source diff; preserve
  local originals. Keep reusable fixtures/tooling, not one-off execution archives.
- Follow the scoped semantic-title convention in README; include verified related
  ticket IDs when applicable and do not imply incomplete tickets are fully resolved.

## OpenSpec workflow

Before planning or implementing an issue, read its available description and every
page of accessible timestamped comments, including inline comments and linked
clarifications. Use public GitHub discussions, repo specs/docs and task-provided
requirements; also review private Linear chronology when already accessible.
Private boards, Codex/connectors and a development tablet are not contributor
prerequisites. Maintainers must copy relevant acceptance criteria and decisions
into public issues, PRs or specs. Note genuinely unavailable context once, proceed
with available requirements, and ask a focused clarification only when necessary;
do not repeatedly request integrations, installation or private credentials.
Reconcile additions and superseded requests in chronological order;
do not assume the description contains the full scope. Map each applicable request
to a task and acceptance check. Re-read new comments before review and closure.
Reviewers must check this coverage as well as code/spec agreement. Track confirmed
omissions as actionable work; documenting an omission does not complete it.

Use the repo-local skills in `.codex/skills/openspec-*` when available for planned work:
propose the full change (proposal, design, delta specs, tasks), apply, verify,
sync canonical specs, and archive in the same implementation PR. See
[the workflow guide](openspec/README.md). Do not split feature planning into a
prerequisite planning-only PR. REM-27 is the deliberate baseline-only exception.
The equivalent public/manual workflow is to create proposal.md, design.md, delta
specs and tasks.md under openspec/changes, implement and verify the requirements,
sync openspec/specs, then move the completed change into changes/archive in the
same PR. The OpenSpec CLI can validate these files but Codex is not required.
Current available issue scope and source behavior outrank historical plans. Record actual
test evidence in PR comments, and complete the issue's acceptance gates before merge.

## Simulator learning

Prefer supported completion events plus verified operation/page/session
postconditions for Reader/Writer sequencing. Where events are unavailable, use
paced fresh-state polling with monotonic deadlines and cancellation; elapsed time
alone is not success. Keep input serialized and ownership/recovery guards intact.
Document the reason, scope, bound and validation for necessary fixed waits;
gesture qualification, animation, polling and timeout timing are distinct from
completion assumptions. Test delayed/lost/stale/repeated signals and cancellation
through the real operation path. Public/manual workflows remain supported; do not
require private integrations or invasive tablet dependencies. The final integrated
performance gate must audit later features for preservation of these properties.

For each device/workflow change or native finding, check its simulator impact.
Update the shared model, fixtures, faults or assertions as applicable in the same
implementation PR, including OpenSpec simulator deltas when behavior changes.
Preserve discovered failure cases as regressions. Distinguish modeled behavior
from hardware or vision proof; document specific limitations when a finding
cannot be faithfully simulated. Unrelated changes need no artificial simulation.

## Tablet access

For tablet compatibility, integration, or end-to-end Reader Buddy testing, when
full unattended SSH is available to an authorized development tablet the user is
not actively using, use
[reader-buddy-testing](.agents/skills/reader-buddy-testing/SKILL.md). It covers
device coordination, realistic test inputs and evidence. Unrelated source or
documentation edits do not require the tablet workflow. The unattended workflow
does not apply to actively used/personal tablets or without full unattended SSH.
Local simulator tests require neither SSH nor a tablet. Contributors without
authorized hardware should run local checks and identify unverified native cases;
a maintainer performs required hardware gates before merge. Never claim unavailable
hardware evidence was verified. Skills should offer equivalent public CLI/file
steps where feasible rather than require a particular assistant or connector.

## Local development testing

Use [.agents/skills/reader-simulator-testing/SKILL.md](.agents/skills/reader-simulator-testing/SKILL.md)
for local scenario selection, deterministic regressions and authorized live-model
checks. It is also a manual checklist. Offline development requires no private
board, assistant integration, API credential or tablet. Model-facing changes still
need representative live evidence before merge; contributors without access report
the missing gate and a maintainer supplies it. Native-only evidence follows the
separate unattended tablet scope above. Existing authorization does not need to be
requested again merely because a skill is used.

## Predictable device interaction

Normal Reader, Writer and future features must not autonomously navigate device
menus, including opening menus merely to inspect settings. Prefer supported direct
interfaces, verified simple gestures or a safe documented fallback. Simple left/right
page swipes are allowed; this rule does not prohibit keyboard text input or all
coordinate-based input. Do not hide menu automation behind helpers or optional
fallbacks. If a feature requires menus, document the limitation and resolve the
design explicitly before adding it. Deliberate developer/manual test setup may
operate menus with advance notice, but must remain separate from product paths
and be labeled clearly in evidence. REM35 audits integrated features for this rule.

When the no-menu rule blocks an operation, investigate relevant current open-source
reMarkable community implementations before concluding it is unavailable. Keep the
research targeted to that roadblock. Inspect actual code, issues/releases and
firmware compatibility; record links/revisions and distinguish supported direct
interfaces, native/file-format mechanisms, injected extensions and UI automation.
Evaluate safety, maintenance and scope costs; do not silently install invasive
dependencies or treat saved preferences as actual UI state. Record promising
alternatives and unresolved limits rather than conducting an endless survey.
