# OpenSpec workflow

Canonical contracts live in `specs/`; completed changes live in `changes/archive/`.
The REM-27 baseline describes v0.1.4 source behavior, including known defects.
It is not a promise that every code path has been tested on every device.

Use the optional Codex skills in `.codex/skills/` in this order, or perform the
equivalent file/CLI workflow below. Neither Codex nor private Linear access is
required to contribute. Read the public issue/PR discussion and task requirements;
maintainers copy relevant private decisions into public acceptance criteria.

1. `openspec-propose`: name the change for the current issue, inspect source
   and canonical specs, and create proposal, design, delta specs and tasks before
   implementation. `openspec instructions <artifact> --change <name> --json`
   supplies the schema and project rules.
2. `openspec-apply-change`: implement the named change, updating task completion
   as work is verified. Keep artifacts aligned when evidence changes the design.
3. `openspec-verify-change`: compare tasks, requirements and scenarios with code
   and tests. Resolve findings; report hardware/model limitations honestly.
4. `openspec-sync-specs`: apply the named delta to `specs/`, preserving unrelated
   requirements. Verify canonical specs match the final implementation: replace modified blocks in place, preserve unaffected requirements, and check for unique requirement headings and no delta-only markers.
5. `openspec-archive-change`: archive that completed, synced change in the same
   implementation PR. The CLI equivalent after explicit sync is
   `openspec archive <name> --skip-specs --yes`; never use this to skip required sync.

Validate with `openspec validate --all --strict --no-interactive`. Archived
artifacts remain part of the PR alongside canonical specs and implementation.
Use one feature PR, not a planning-only prerequisite PR. REM-27 is intentionally
documentation-only because establishing the baseline is its actual deliverable.
Do a fresh final review and required CI/issue acceptance checks before merging.
Follow AGENTS.md: concise Summary-only PR body; results/screenshots in comments.

## Manual equivalent

Create `changes/<name>/proposal.md` (why/scope), `design.md` (decisions/risks),
`specs/<capability>/spec.md` (added/modified requirements with WHEN/THEN scenarios),
and `tasks.md` (acceptance checklist) before implementation. Read all accessible
issue comments chronologically and map each applicable request to acceptance.
Implement, run public local tests and compare every requirement to evidence.
Update canonical `specs/` with the final requirements, then move the completed
change to `changes/archive/YYYY-MM-DD-<name>/` in the same PR. The optional
OpenSpec CLI validates the artifacts; unavailable integrations are not a reason
to block ordinary contributions. Clearly mark hardware or live-model cases that
could not be run and have a maintainer complete required gates before merge.

## Official skill provenance

Generated with the installed `@fission-ai/openspec` CLI 1.2.0 using
`openspec init --tools codex`. Core skills are propose, explore, apply and archive.
The verify and sync skills use the unmodified instruction bodies exported by
`getVerifyChangeSkillTemplate` and `getSyncSpecsSkillTemplate` from that same
official package's `dist/core/templates/skill-templates.js`, with the same MIT
skill metadata wrapper. This adds the required workflows without modifying global
OpenSpec settings. On upgrade, regenerate these two from the official package
alongside core skills and review the diff; do not replace them with invented steps.

Each bundled skill now carries a clearly separated Repository portability note and optional-CLI compatibility metadata. These project additions qualify unavailable-tool and private-access assumptions; the upstream workflow body remains attributable to the generator. Preserve the note when regenerating and review regenerated instructions against the public manual workflow.

Project rules live in `config.yaml`. No runtime secrets belong in spec artifacts.
Writer Buddy and other planned capabilities get their own specs when implemented;
do not baseline unimplemented roadmap promises as existing behavior.
