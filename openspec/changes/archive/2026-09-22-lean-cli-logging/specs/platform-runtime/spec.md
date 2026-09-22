## MODIFIED Requirements

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
