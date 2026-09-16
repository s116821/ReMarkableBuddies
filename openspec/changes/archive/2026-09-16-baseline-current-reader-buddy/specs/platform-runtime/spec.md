## ADDED Requirements

### Requirement: Runtime startup and configuration
The executable SHALL load an optional .env file before argument parsing, default to model gpt-5.6-terra and trigger corner LL, and accept OPENAI_API_KEY and OPENAI_BASE_URL or their CLI overrides. Invalid corner values SHALL fail startup. Source: src/main.rs Args/main; src/llm/openai.rs new/from_env.

#### Scenario: Normal initialization
- **WHEN** normal execution starts with usable credentials and device access
- **THEN** it initializes cache/capture/input, waits 1000 ms for devices, constructs the OpenAI client, and loops unless --once selects a single iteration.

#### Scenario: Missing credentials
- **WHEN** no API key is supplied for normal execution
- **THEN** initialization fails when constructing the model client, after workflow/device initialization.

### Requirement: Existing diagnostic flags
The CLI SHALL expose --screenshot-only FILE, --api-key, --model/-m, --base-url, --no-draw, --no-trigger, --once, --input-png, --save-screenshot, --trigger-corner, --log-level and --debug-dump. In this baseline --input-png and --save-screenshot SHALL remain parsed but unused; they do not substitute or save captures. Source: src/main.rs Args/main. This records a known gap, not desired future behavior.

#### Scenario: Capture-only execution
- **WHEN** --screenshot-only FILE is supplied
- **THEN** capture is saved and the process exits before credential validation or input device initialization.

#### Scenario: Disabled drawing is not a simulator
- **WHEN** --no-draw is supplied
- **THEN** pen/keyboard/touch device handles are disabled but screenshot capture and model work are not replaced; waiting for a disabled Linux touch trigger returns an error.

#### Scenario: Trigger bypass
- **WHEN** --no-trigger and --once are supplied
- **THEN** the single iteration starts capture immediately without waiting for a gesture.

### Requirement: Logging and service lifecycle
The runtime SHALL use env_logger with millisecond timestamps, RUST_LOG filtering and fallback --log-level info. The supplied service SHALL run /opt/bin/reader-buddy from /home/root, require the configured environment file, write stdout/stderr to the journal and restart on failure after five seconds. Source: src/main.rs; deploy/reader-buddy.service.

#### Scenario: Service configuration
- **WHEN** the supplied systemd unit is used
- **THEN** it reads /home/root/.config/reader-buddy/environment and orders after home.mount, xochitl.service and network-online.target.

#### Scenario: Debug data exposure
- **WHEN** verbose logging or debug dumps are enabled
- **THEN** model request/response content and page images can appear in logs or /tmp; these diagnostics are not a secret-safe telemetry subsystem.
- **AND** API authorization headers are not included in the explicit request-body log.

### Requirement: Implemented product boundary
The normal application SHALL process independent Reader Buddy iterations only. It SHALL NOT implement Writer Buddy, follow-up conversation history, document retrieval, external search tools, persistent subject memory, handwriting personalization, cloud sync, native answer-page creation, or a maintained simulator in this baseline. Source: src/main.rs; src/workflow/orchestrator.rs; src/llm/openai.rs.

#### Scenario: New question after previous answer
- **WHEN** another iteration starts
- **THEN** model content is rebuilt from the current page rather than a retained conversation or document corpus.
