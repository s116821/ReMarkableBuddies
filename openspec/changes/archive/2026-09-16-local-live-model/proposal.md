## Why

REM-22 established deterministic local Reader execution, but the laptop has no configured LLM credentials and scenarios only accept scripted replies. REM-28 enables representative live-model development without routine tablet access.

## What Changes

- Add an explicit live model choice within structured scenarios, retaining scripted/offline defaults and the existing OpenAI abstraction.
- Bound live request count and request duration, report the selected model mode, and preserve PNG/JSON evidence on assertion failures.
- Document portable environment/.env configuration and a representative live Reader scenario; configure the authorized laptop without committing secrets.

## Capabilities

### New Capabilities
- `local-model-configuration`: explicit live development configuration, secret handling and bounded invocation.

### Modified Capabilities
- `local-simulator`: share execution/reporting between scripted and live model adapters and distinguish evidence fidelity.
- `platform-runtime`: clarify credential requirements for explicit live scenarios.

## Impact

Simulator schema/model adapter, OpenAI client timeout configuration, local setup documentation and fixture, offline tests. No tablet input/capture behavior or model prompts change. No new provider or dependency.
