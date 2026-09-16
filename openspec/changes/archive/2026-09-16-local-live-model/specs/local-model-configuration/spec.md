## ADDED Requirements

### Requirement: Explicit local provider configuration
Live simulator execution SHALL require explicit llm mode live, use the existing OpenAI LLMEngine with OPENAI_API_KEY and optional OPENAI_BASE_URL, and support an optional scenario model override. Missing or blank credentials SHALL fail before provider access. Documentation SHALL describe ignored .env/environment setup and required secret permissions without storing credentials in tracked files. Source: src/simulator; src/llm/openai.rs; docs/local-development.md.

#### Scenario: Missing credentials
- **WHEN** an explicit live scenario has no nonblank API key
- **THEN** execution fails with a configuration error without invoking a provider.

#### Scenario: Configured laptop
- **WHEN** an explicit live Reader scenario has valid local configuration
- **THEN** the production Reader prompt, verification and rendering flow uses the configured provider and exports a report labeled live-provider.

### Requirement: Bounded live requests
Live scenarios SHALL validate max_calls in 1..200 and timeout_seconds in 1..300, defaulting to 2 calls and 90 seconds per request. They SHALL reject scripted replies and blank explicit model names. The adapter SHALL enforce the call limit before provider invocation and apply the request timeout. Source: src/simulator/scenario.rs; src/simulator/mod.rs; src/llm/openai.rs.

#### Scenario: Exhausted allowance
- **WHEN** a workflow attempts a provider request beyond max_calls
- **THEN** it returns a bounded error without making that request and retains execution evidence.

#### Scenario: Invalid mode configuration
- **WHEN** a live scenario includes scripted replies or invalid bounds
- **THEN** schema validation fails before device or provider initialization.

### Requirement: Attributable development evidence
Reports SHALL identify live versus scripted model execution and SHALL NOT serialize API credentials. Live acceptance SHALL verify a representative Reader request, source preservation and output while explicitly distinguishing approximate simulator rendering from native hardware and live answer variability from deterministic regressions. Source: src/simulator/mod.rs; docs/local-development.md.

#### Scenario: Live acceptance
- **WHEN** the configured laptop runs the representative paper-question scenario
- **THEN** the result records actual provider calls and answer output with source preservation and a labeled model mode.
