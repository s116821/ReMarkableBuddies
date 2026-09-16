# platform-runtime

## Purpose

Describe the implemented platform runtime contracts, initially baselined from v0.1.4. Known gaps are explicit and require a later change delta to alter.

## Requirements

### Requirement: Runtime startup and configuration
The executable SHALL load an optional .env file before argument parsing, default to model gpt-5.6-terra and trigger corner LL, and read the API key only from OPENAI_API_KEY, with OPENAI_BASE_URL or --base-url selecting the endpoint. Invalid corner values SHALL fail normal workflow startup; capture-only execution exits before workflow configuration. Source: src/main.rs Args/main; src/llm/openai.rs new/from_env.

#### Scenario: Normal initialization
- **WHEN** normal execution starts with usable credentials and device access
- **THEN** it validates credentials/configuration before device initialization, initializes cache/capture/input, waits 1000 ms for devices, and loops unless --once selects a single iteration.

#### Scenario: Missing credentials
- **WHEN** no API key is supplied for normal execution
- **THEN** initialization fails before workflow/device initialization.

### Requirement: Existing diagnostic flags
The CLI SHALL expose only --simulate SCENARIO, --screenshot-only FILE, --model/-m, --base-url, --no-trigger, --once and --trigger-corner plus help/version. Removed --input-png, --save-screenshot, --no-draw, --api-key, --log-level and --debug-dump switches SHALL be rejected. Simulation SHALL be mutually exclusive with capture-only and normal workflow overrides. Source: src/main.rs Args/main.

#### Scenario: Capture-only execution
- **WHEN** --screenshot-only FILE is supplied
- **THEN** capture is saved and the process exits before credential validation or input device initialization.

#### Scenario: Explicit diagnostic capture configuration
- **WHEN** normal workflow startup reads READER_BUDDY_DEBUG_DUMP set to true or 1
- **THEN** optional local image dumps are enabled; absent, false or 0 disables them, and other values fail configuration validation before device access.

#### Scenario: Trigger bypass
- **WHEN** --no-trigger and --once are supplied
- **THEN** the single iteration starts capture immediately without waiting for a gesture.

#### Scenario: Local scenario
- **WHEN** --simulate SCENARIO is supplied
- **THEN** the structured bounded scenario executes without initializing real devices or requiring an API key.

### Requirement: Logging and service lifecycle
The runtime SHALL use env_logger with millisecond timestamps, RUST_LOG filtering and fallback info. The supplied service SHALL run /opt/bin/reader-buddy from /home/root, require the configured environment file, write stdout/stderr to the journal and restart on failure after five seconds. Source: src/main.rs; deploy/reader-buddy.service.

#### Scenario: Service configuration
- **WHEN** the supplied systemd unit is used
- **THEN** it reads /home/root/.config/reader-buddy/environment and orders after home.mount, xochitl.service and network-online.target.

#### Scenario: Debug data exposure
- **WHEN** verbose logging or debug dumps are enabled
- **THEN** normal logs include question/answer text, debug logs include parsed model responses, and page images appear in explicitly enabled /tmp dumps; request diagnostics do not log the entire image-bearing request body.
- **AND** explicit request diagnostics do not include API authorization headers.

### Requirement: Implemented product boundary
The application SHALL process independent Reader Buddy iterations on real devices or the maintained simulator. It SHALL NOT yet implement Writer Buddy, follow-up conversation history, document retrieval, external search tools, persistent subject memory, handwriting personalization, cloud sync or native answer-page creation. Source: src/main.rs; src/workflow/orchestrator.rs; src/llm/openai.rs; src/simulator.

#### Scenario: New question after previous answer
- **WHEN** another iteration starts
- **THEN** model content is rebuilt from the current page rather than a retained conversation or document corpus.
