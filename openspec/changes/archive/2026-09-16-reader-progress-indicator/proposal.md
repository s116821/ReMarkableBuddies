## Why

REM-8 gives immediate visible feedback while Reader works, especially during model latency. Today the service appears idle until it writes an answer or draws a large failure X.

## What Changes

- Draw and refresh a circle in a 50 by 50 bottom-right region on active pages while Reader processes a request.
- Clear temporary marks before capture, navigation, typing, completion or failure; draw failure X within the same region.
- Keep all tablet input on the workflow thread while bounded HTTP work runs separately during model waits.
- Preserve existing corner content: suppress only the indicator/failure mark if the region cannot safely be used, continuing Reader processing.
- Add deterministic lifecycle/failure tests, live wait evidence and native cleanup/preservation validation.

## Capabilities

### New Capabilities
- reader-progress: visible activity, safe occupied-region fallback, serialized input and cleanup lifecycle.

### Modified Capabilities
- reader-answer-pages: failure X size, cleanup and safe suppression.
- reader-analysis: bounded model request wait with progress callbacks.
- local-simulator: indicator state/events and fault/lifecycle assertions.

## Impact

Workflow, device backend/pen, LLMEngine/OpenAI wait mechanism, simulator fixtures/reporting and docs/specifications. No new model provider, page insertion, gesture changes or background thread with device access. No promise of native ink recovery after an abrupt process kill or concurrent manual input.
