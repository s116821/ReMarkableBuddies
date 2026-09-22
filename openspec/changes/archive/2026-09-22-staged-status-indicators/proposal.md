## Why

REM-8's later comments replaced the initial single circle with staged triangles and six failure codes. The current implementation and specs still describe the superseded circle, so activity does not identify workflow progress or failure cause.

## What Changes

- Draw three nested triangle stages one edge every 333 ms, retracing the active triangle while it remains pending.
- Draw an inscribed circle tangent to the innermost triangle during auxiliary inference, including independent transcription.
- Pair the constant failure X with one of six documented segments for selection, transcription, provider, no-successor, invalid-successor and device failures.
- Track bounded owned paths and clear them before capture, navigation, viewport-changing output and completion. Preserve occupied corners, selected tools and strict history ownership.
- Model geometry, stage transitions, timing and fault cleanup in the simulator and validate native output.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `reader-progress`: staged geometry, cadence, auxiliary inference and complete owned-path cleanup.
- `reader-answer-pages`: six guarded persistent failure marks instead of generic X or loop-mode error text.
- `reader-analysis`: distinguish unavailable verification from transcription disagreement while retaining conservative rejection and error propagation.
- `local-simulator`: shared geometry and deterministic stage/timing/error observations.

## Impact

Workflow, device backend, OpenAI progress scheduling, simulator, tests and contributor documentation. No new dependency, firmware change, account pairing, retrieval implementation or relaxation of native history guards. REM-4 retains context-enhancement spokes as its own integration task.
