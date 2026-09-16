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
- **THEN** the structured bounded scenario executes without initializing real devices; scripted mode requires no API key, while explicit live mode requires provider credentials.


