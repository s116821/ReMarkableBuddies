## Context

The maintained simulator executes production Reader orchestration with a ScriptedModel. CLI startup loads .env, while the real runtime uses OPENAI_API_KEY and optional OPENAI_BASE_URL. The laptop currently has neither configured.

## Goals / Non-Goals

Goals: opt-in live Reader execution on simulated pages, reproducible trusted-machine secret setup, bounded requests and attributable output.
Non-goals: provider migration, prompt changes, physical tablet validation, Writer, native insertion, or deterministic claims about live answers.

## Decisions

- Add a tagged scenario llm configuration: absent or scripted remains offline; live uses the existing OpenAI LLMEngine with an optional model override, max_calls (default 2) and timeout_seconds (default 90). Reject live replies, empty model names and out-of-range bounds before execution. A new global CLI flag was rejected because the structured scenario already selects the workflow.
- Factor one generic execution/report function; scripted and live wrappers share it. Live wrapper delegates content and records requests/responses without credentials; enforces the request limit before a provider call. The existing OpenAI client gains an opt-in timeout builder, leaving normal tablet defaults unchanged.
- Live config reads OPENAI_API_KEY and optional OPENAI_BASE_URL from environment or the existing ignored .env loader. Missing/blank credentials fail before execution/network. Keep secrets outside tracked content; use a restricted local .env for this trusted laptop. Document manual setup on another machine and shell permissions. No committed key, private endpoint or workstation path.
- Reports label live-provider vs scripted-offline. Exact text assertions remain available, with substring assertions for stable semantic anchors in nondeterministic answers. Export PNG/JSON before assertion errors as before. CI exercises configuration and a fake model/local HTTP endpoint, never paid external calls.
- Maintain a separate explicit live example using the existing cursive paper screenshot and blank successor. Verify expected calls/navigation/preserved source, manually assess scientific answer and inspect raster output. Native typography still needs hardware.

## Risks / Trade-offs

- Live calls incur costs and send page content to the configured provider -> explicit live scenario, documented data flow, finite request count and timeout, existing user authorization for this representative run.
- Local .env contains credentials -> ignored file, restricted permissions, values never printed or committed; existing process environment takes precedence.
- Live answer variability -> assert stable invariants, inspect scientific meaning, distinguish from offline reproducibility.
- Shared refactor could alter offline results -> retain all 25 scenarios and deterministic pixel/report checks.

## Migration Plan

Existing scenarios and tablet invocation remain compatible. Implement and run offline checks, configure the laptop, run the live acceptance example, sync/archive and review in one PR. No tablet deployment needed for the simulator-only behavior; ARM builds verify shared client compatibility.

## Open Questions

None.
