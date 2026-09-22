# platform-runtime

## Purpose

Describe the implemented platform runtime contracts, initially baselined from v0.1.4. Known gaps are explicit and require a later change delta to alter.

## Requirements

### Requirement: Runtime startup and configuration
The executable SHALL load an optional .env before argument parsing, default to model gpt-5.6-terra and corner LL, and support --api-key with OPENAI_API_KEY fallback and --base-url with OPENAI_BASE_URL fallback. Explicit CLI values SHALL take precedence. Normal configuration SHALL be validated before device initialization; absent or blank selected credentials SHALL fail without printing their values. Source: REM6/REM28/REM34; src/main.rs.

#### Scenario: Normal initialization
- **WHEN** usable normal configuration and device access are available
- **THEN** the runtime initializes cache/capture/input, waits 1000 ms for devices, then waits for triggers in its continuous loop.

#### Scenario: Missing or blank credentials
- **WHEN** no usable selected credential is supplied
- **THEN** startup fails before device initialization with a value-free error.

### Requirement: Existing diagnostic flags
The production CLI SHALL expose only --api-key, --model/-m, --base-url, --trigger-corner, --log-level, --debug-dump and --simulate plus help/version. Other production flags SHALL be rejected. Simulation SHALL reject explicit normal key/model/endpoint/corner/dump overrides, while allowing logging control. Screenshot-only and one immediate native iteration SHALL remain available as explicitly built diagnostic examples using production components, outside the production CLI and distributed archives. Source: REM6/REM22/REM34.

#### Scenario: Image dump configuration
- **WHEN** --debug-dump is present
- **THEN** image dumps are enabled; otherwise READER_BUDDY_DEBUG_DUMP true/1 enables, absent/false/0 disables, and invalid values fail before device initialization.

#### Scenario: Offline scenario
- **WHEN** --simulate selects a scripted scenario
- **THEN** it executes without real devices or credentials and preserves exact declared workflow assertions; scenario live mode remains explicit.

#### Scenario: Bounded development tools
- **WHEN** the screenshot example is invoked with its output path
- **THEN** it captures without input initialization or credentials; the separate reader_once example runs one immediate production iteration using environment credentials and default model/corner.

### Requirement: Logging and service lifecycle
The runtime SHALL use env_logger with millisecond timestamps. --log-level SHALL validate off/error/warn/info/debug/trace and override RUST_LOG; absent explicit control SHALL use RUST_LOG, otherwise info globally and debug for Reader Buddy application targets. No dedicated debug enable toggle SHALL be required. The supplied service SHALL retain /opt/bin/reader-buddy from /home/root, its protected environment file, journal output and five-second restart-on-failure policy. Source: REM14/REM34; src/main.rs; deploy/reader-buddy.service.

#### Scenario: Useful diagnostics by default
- **WHEN** no explicit logging override is set
- **THEN** Reader Buddy debug messages are enabled while dependency debug messages are disabled, and image dumps remain independently opt-in.

#### Scenario: Service configuration
- **WHEN** the supplied unit is used
- **THEN** it reads /home/root/.config/reader-buddy/environment and orders after home.mount, xochitl.service and network-online.target.

#### Scenario: Debug data exposure
- **WHEN** default debug diagnostics are emitted
- **THEN** logs may include question/answer and parsed model text but explicit request diagnostics exclude authorization headers and entire image-bearing request bodies; page images are saved only with dump opt-in.

### Requirement: Implemented product boundary
The application SHALL process independent Reader Buddy iterations on real devices or the maintained simulator. It SHALL NOT yet implement Writer Buddy, follow-up conversation history, document retrieval, external search tools, persistent subject memory, handwriting personalization, cloud sync or native answer-page creation. Source: src/main.rs; src/workflow/orchestrator.rs; src/llm/openai.rs; src/simulator.

#### Scenario: New question after previous answer
- **WHEN** another iteration starts
- **THEN** model content is rebuilt from the current page rather than a retained conversation or document corpus.

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
