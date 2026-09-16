## Why

REM-13 consolidates baseline cleanup before navigation fixes and simulator work.
Inert CLI switches and inline workflow decisions make regressions difficult to test.

## What Changes

- Remove unused input/save-image flags and misleading no-draw CLI mode; retain bounded
  screenshot-only, once and trigger-bypass controls needed for real-device diagnostics.
- Use RUST_LOG instead of --log-level and environment configuration for optional
  image dumps instead of a dedicated CLI debug toggle. Keep secrets out of argv.
- Extract reusable page verification/classification and Q&A composition.
- Isolate hold timing for behavioral tests and replace model response panics with errors.
- Add meaningful config, navigation, rendering, gesture and response regressions;
  preserve strict lint CI and require final real-tablet smoke before merge.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `platform-runtime`: lean configuration surface and standard logging.
- `reader-analysis`: malformed model responses return recoverable errors.
- `reader-answer-pages`: reusable behavior-preserving navigation and composition boundaries.
- `tablet-io`: hold timing protected by deterministic behavioral tests.

## Impact

Rust configuration, device/workflow helpers, regression tests and documentation.
REM-10 still owns changing return retries; REM-22 owns the simulator. No new product
features, provider integrations, firmware changes or account sync.
