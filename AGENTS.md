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

Use the official repo-local skills in `.codex/skills/openspec-*` for planned work:
propose the full change (proposal, design, delta specs, tasks), apply, verify,
sync canonical specs, and archive in the same implementation PR. See
[the workflow guide](openspec/README.md). Do not split feature planning into a
prerequisite planning-only PR. REM-27 is the deliberate baseline-only exception.
Current Linear scope and source behavior outrank historical plans. Record actual
test evidence in PR comments, and complete the issue's acceptance gates before merge.

## Tablet access

For tablet compatibility, integration, or end-to-end Reader Buddy testing, when
full unattended SSH is available to an authorized development tablet the user is
not actively using, use
[reader-buddy-testing](.agents/skills/reader-buddy-testing/SKILL.md). It covers
device coordination, realistic test inputs and evidence. Unrelated source or
documentation edits do not require the tablet workflow. The unattended workflow
does not apply to actively used/personal tablets or without full unattended SSH.
