## Context

REM22 comment674a9fc1 (2026-09-15 21:03:13 UTC) requests a separate simulator skill and real LLM testing. REM14 commentf4e309cb (20:55:03 UTC) requests diagnostics in unattended guidance. REM33's September22 portability clarification makes public requirements/manual workflows sufficient for contributions, with maintainer live/native gates before merge. Full descriptions and all comment pages were read: REM33 none, REM22 one, REM14 one, REM28 one. REM28 documents an ignored local credential location; that developer-specific path is not copied into public instructions.

## Goals / Non-Goals

Goals: discoverable independent local workflow, honest evidence, useful fixtures/assertions, secret-safe diagnostics and portable contribution rules. Non-goals: runtime logging-default or CLI changes, Writer implementation, tablet access expansion, new provider setup or changing OpenSpec architecture.

## Decisions

Create `.agents/skills/reader-simulator-testing/SKILL.md`, linked from AGENTS and simulator docs. Keep it self-contained and concise, reusing maintained simulator schema/local development/fixture documentation rather than duplicating it. It also functions as a manual checklist. Deterministic runs work without keys/SSH and cannot prove vision. Model-facing changes require representative authorized live checks; contributors without access report the gap and a maintainer supplies evidence before merge. Existing task authorization persists; Codex usage restrictions are distinct from application API authorization.

Select fixtures by changed behavior, include exact output/forbidden-operation assertions and negative cases, and inspect PNG+report artifacts rather than exit status alone. Use cursive/shorthand, ambiguous/absent question and occupied/recovery/history/status cases where applicable. Preserve failures, avoid easier replacement-only claims and extend simulation from native findings in the same implementation PR.

Link native skill to maintained journal guidance, including bounded time/unit/output selection and RUST_LOG emission. Do not assume journal priority filters map env_logger severity. Default logging currently info; changing that belongs to REM34. Set debug environment only for an authorized run and preserve prior service state.

Public requirements come from GitHub discussions/specs/task input, plus complete timestamped private comments when already available. State missing context once; do useful work and ask only necessary clarification. Agent-specific actions have equivalent file/CLI/manual steps. Add a concise project portability note to bundled OpenSpec skills where needed and preserve the upstream body/provenance distinction. Verify sync replaces modified blocks in-place, preserves unrelated requirements and has no duplicate headings/delta markers.

## Risks / Trade-offs

Documentation may drift from flags/schema: run a maintained offline positive and negative scenario and check declared assertions/output. A frontmatter validator is structural only; exercise realistic no-credentials and authorized-live routing cases without network/device side effects. Avoid new tests that merely mirror prose.

## Migration Plan

No runtime migration. Implement docs/skills after full artifacts, verify links/frontmatter/offline examples, sync canonical contracts and archive in the same documentation PR. Required CI and independent review remain delivery gates. Pure documentation merge must not create an application release under REM30 policy.
