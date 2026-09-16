## MODIFIED Requirements

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

### Requirement: Implemented product boundary
The application SHALL process independent Reader Buddy iterations on real devices or the maintained simulator. It SHALL NOT yet implement Writer Buddy, follow-up conversation history, document retrieval, external search tools, persistent subject memory, handwriting personalization, cloud sync or native answer-page creation. Source: src/main.rs; src/workflow/orchestrator.rs; src/llm/openai.rs; src/simulator.

#### Scenario: New question after previous answer
- **WHEN** another iteration starts
- **THEN** model content is rebuilt from the current page rather than a retained conversation or document corpus.
