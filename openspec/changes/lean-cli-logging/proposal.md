## Why
REM34 corrects missed REM6 CLI and REM14 default diagnostic requirements. Historical agent-authored foundation specs do not supersede those user requests; REM22 separately authorizes the simulator selector.

## What Changes
- Restore optional api-key, log-level and debug-dump controls, preserving environment credentials and explicit configuration precedence.
- Remove production no-trigger, once and screenshot-only switches; preserve equivalent bounded diagnostics as development examples.
- Default to Reader Buddy debug messages with quieter dependency logs; retain RUST_LOG and ordinary journalctl use.
- Update invocation guidance and meaningful configuration/simulator regressions in this PR.

## Capabilities
### New Capabilities
None.
### Modified Capabilities
- `platform-runtime`: requested CLI, environment precedence, logging and example diagnostics.

## Impact
src/main.rs, diagnostic examples, config/parser tests, README/technical/env/testing guidance. Normal systemd invocation stays unchanged. This is a corrective pre-1.0 interface migration; it does not declare product 1.0 or change prompts, history, gesture or rendering policies.
