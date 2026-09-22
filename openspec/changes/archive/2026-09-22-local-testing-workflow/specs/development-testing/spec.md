## ADDED Requirements

### Requirement: Discoverable portable development workflow
The repository SHALL provide a separate local simulator testing skill linked from AGENTS and simulator documentation. It SHALL also be usable as a manual checklist with ordinary repository files and CLI tools. Public GitHub discussions, specifications and supplied task requirements SHALL support contributors without private board or agent access; maintainers with accessible private chronology SHALL read complete timestamped comments and publish relevant acceptance criteria. Missing optional tools or private context SHALL be stated once, without repeated credential/integration requests, while useful independent work continues. Source: REM33; AGENTS.md; .agents/skills/reader-simulator-testing/SKILL.md; openspec/README.md.

#### Scenario: Contributor without private integrations
- **WHEN** a contributor has the repository and public task but no Linear/Codex/tablet access
- **THEN** they can select and execute offline scenarios, edit artifacts and submit their contribution using manual equivalents, with genuinely missing acceptance evidence clearly identified.

#### Scenario: Optional agent tool unavailable
- **WHEN** a bundled skill names a tool unavailable in the contributor's environment
- **THEN** equivalent conversation, file or CLI steps preserve the workflow outcome without requiring that tool or silently omitting required review/spec artifacts.

### Requirement: Evidence and authorization boundaries
Development guidance SHALL distinguish deterministic scripted behavior, real-model interpretation and native device behavior. Model-facing changes SHALL receive representative authorized live validation before merge; unavailable contributor access SHALL become a maintainer verification responsibility. Existing task authorization SHALL remain effective without repeated approval, and application API costs SHALL remain distinct from assistant capacity restrictions. Secrets SHALL stay in ignored local/process configuration and never be printed, included in scenarios or committed. Source: REM22 comment674a9fc1; REM28; docs/local-development.md; .agents/skills/reader-simulator-testing/SKILL.md.

#### Scenario: Offline execution
- **WHEN** a scripted scenario is selected without credentials or SSH
- **THEN** local execution and exact assertions remain available and its evidence does not claim model handwriting recognition or hardware fidelity.

#### Scenario: Model-facing acceptance unavailable locally
- **WHEN** a contributor changes prompts/model interpretation but lacks live access
- **THEN** they preserve local results and the missing live gate, and a maintainer supplies appropriate authorized live evidence before merge rather than treating scripts as vision proof.

### Requirement: Scoped native diagnostics
Unattended tablet guidance SHALL link or describe bounded journalctl unit/time/output inspection and RUST_LOG application emission, while preserving its authorized idle development tablet scope and original service/environment state. Journal priority filtering SHALL not be presented as guaranteed Rust log-level filtering. Logs and debug images SHALL be reviewed for sensitive content before intentional publication. Source: REM14 commentf4e309cb; .agents/skills/reader-buddy-testing/SKILL.md; README.md.

#### Scenario: Authorized native investigation
- **WHEN** an authorized idle tablet run needs diagnostics
- **THEN** the maintainer can capture bounded relevant logs, enable scoped debug emission if needed and restore prior service state without broadening authorization to firmware, accounts or a personal tablet.
