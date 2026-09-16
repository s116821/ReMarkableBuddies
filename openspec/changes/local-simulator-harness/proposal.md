## Why

REM-22 removes routine tablet dependence from Reader regression testing. The current non-Linux stubs cannot execute realistic page workflows or reproduce recovery failures, slowing every later roadmap feature.

## What Changes

- Introduce one shared device interface used by real hardware and a stateful local simulator; run the existing Reader orchestrator with the existing model interface.
- Add one structured `--simulate SCENARIO` entry point with deterministic replies, page/gesture/failure fixtures, virtual time, assertions and PNG/JSON output.
- Cover accepted/rejected questions, blank and existing answers, occupied successors, end pages, failed navigation/capture/rendering and hold behavior.
- Repair the real-adapter prerequisite discovered during validation: find the validated RM2 framebuffer header inside merged anonymous memory maps after a UI restart, without fixed addresses or ambiguous fallback.
- Document measured RM2 assumptions and physical limitations. Writer/combined execution and native insertion remain unsupported until their production roadmap implementations, with simulator extensions explicitly required in REM-23/REM-25/REM-17.

## Capabilities

### New Capabilities
- `local-simulator`: shared-interface deterministic execution, reproducible scenarios and observable results.

### Modified Capabilities
- `platform-runtime`: one simulator selector and explicit product boundary.
- `tablet-io`: validate framebuffer allocations inside merged mappings as well as at mapping starts.

## Impact

Device/workflow boundary, generic model orchestration, CLI, fixtures, integration tests and docs. Hardware input algorithms, model prompts and page decisions remain unchanged; bounded framebuffer discovery covers merged maps. No credentials or new cloud services are introduced.
