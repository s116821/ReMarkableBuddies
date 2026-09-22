## Why

REM22's September15 request for a separate local testing skill was missed. Local development documentation exists, but its discovery and acceptance workflow must be distinct from authorized unattended tablet work. Contributors must be able to use public requirements and manual tools without private boards, agent integrations, API credentials or hardware for offline development.

## What Changes

- Add a repository-local Reader simulator testing skill and discoverable links, routing deterministic fixtures, authorized live-model checks and native-only gates correctly.
- Connect unattended tablet guidance to bounded journalctl/RUST_LOG diagnostics without expanding tablet authorization.
- Audit repository agent/OpenSpec guidance for optional-tool fallbacks and portable public acceptance criteria; retain upstream provenance with explicit project guidance.
- Record same-PR fixture/model extensions for native findings, preserved failed evidence and maintainer completion of unavailable live/native acceptance.

## Capabilities

### New Capabilities
- `development-testing`: portable discovery, local/live/native evidence boundaries and diagnostic workflow.

### Modified Capabilities
- `local-simulator`: contributor workflow expectations and live validation for model-facing changes.

## Impact

Documentation and skills only. No changes to runtime defaults, CLI, prompts, credential handling, production Reader behavior or device services. REM34 separately reconciles runtime logging/CLI chronology. No native mutation or paid model call is necessary to validate this documentation-only change; verify documented offline commands and skill routing, and label prior live evidence separately.
