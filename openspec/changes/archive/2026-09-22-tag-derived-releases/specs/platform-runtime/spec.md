## ADDED Requirements

### Requirement: Git-derived application version
The CLI application version SHALL derive from Git metadata through vergen-gitcl, never from an independently maintained Cargo package version. An official build SHALL require full clean history, the exact expected semantic tag and SHA, and SHALL report that tag's version. Development or unavailable metadata SHALL be identified explicitly. Source: REM-30; build.rs and src/main.rs version configuration.

#### Scenario: Official tagged build
- **WHEN** an official build checks out its verified release tag on either distributed target
- **THEN** --version reports the tag's semantic version and packaging provenance records the same tag and SHA.

#### Scenario: Missing or conflicting official metadata
- **WHEN** an official build has shallow/missing history, dirty source, conflicting overrides or the wrong tag/SHA
- **THEN** the build fails rather than substituting the manifest version or a misleading release version.

#### Scenario: Development build
- **WHEN** a local or PR build is not a clean exact release source or Git metadata is unavailable
- **THEN** --version clearly identifies a development state, including a commit-derived identifier when available.
