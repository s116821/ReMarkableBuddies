## Why

REM-30 replaces a release action that detects feature branches instead of semantic squash messages, producing patch bumps for features merged to main. Releases also compile an implicit checkout and maintain a second version in Cargo, while documentation-only main pushes compile the application.

## What Changes

- Use maintained git-cliff for conventional-commit semantic version computation, including ticket scopes and explicit breaking-change policy.
- Tag the actual merged application commit before compiling its exact source; preserve published tags and remove generated version commits.
- Derive runtime versions from Git through vergen-gitcl, with clearly non-authoritative Cargo metadata and strict official-build verification.
- Share conservative documentation/application classification across main CI and release handling; reject application changes mislabeled as docs.
- Serialize release publication, coalesce queued application changes, and recover incomplete tagged releases on retry.
- Require full timestamped Linear comment review and acceptance mapping before implementation and closure.

## Capabilities

### New Capabilities
- `release-versioning`: semantic tags, source classification, ordered builds and recoverable publication.

### Modified Capabilities
- `platform-runtime`: tag-derived CLI version and explicit development/missing-metadata behavior.

## Impact

Changes GitHub CI/release workflows, release scripts/configuration, Cargo build metadata, CLI version wiring, contributor documentation and isolated release fixtures. No tablet workflow changes or native mutation are needed. Historical tags remain untouched. REM-30 description and complete comment history were reviewed on 2026-09-22; the comment API returned no comments. The separate user clarification permits a non-authoritative manifest placeholder and does not require future registry packaging in this scope.
