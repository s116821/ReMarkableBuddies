## Context

The Reader orchestrator directly owns OpenAI; Workflow directly owns screenshot,
pen, keyboard and touch implementations. Non-Linux stubs cannot model pages.
REM-10 established bounded recovery, and REM-13 isolated classification and holds.

## Goals / Non-Goals

Goals: run the same Reader decisions locally; model pages, output, gestures,
timing and failures; produce deterministic, inspectable regression evidence.
Non-goals: physical framebuffer/input fidelity, handwriting recognition with
scripted replies, native document serialization, Writer or insertion production
features, credential provisioning (REM-28), and general UI emulation.

## Decisions

- Put tablet side effects behind a boxed DeviceBackend owned by Workflow. A real
  adapter delegates existing capture/input/cache methods unchanged. The simulator
  supplies page snapshots, output, cache and virtual waits. Keep policy and image
  classification in Workflow and orchestration, not duplicated in the simulator.
  A second fake orchestrator was rejected because it would not catch real defects.
- Parameterize Orchestrator over the existing LLMEngine with OpenAI as default.
  Scripted replies use the same content-building/parsing/verification path and
  record request counts. No model requests or credentials in deterministic mode.
- One --simulate scenario.json option selects a bounded scenario. Pages refer to
  image fixtures or blank paper; ordered gesture frames use the shared hold timer;
  explicit operation/call fault rules inject error, no movement or stale capture.
  Virtual delays preserve requested durations without wall-clock sleeps. Scenario
  paths resolve relative to the scenario. Reject unknown fields and invalid states.
- Record ordered operations, virtual timestamps, active page, text and X markers;
  export page PNGs plus JSON results even when a workflow fails. Evaluate expected
  page, text, marker, operation/model counts and error assertions; mismatch exits
  nonzero. Image mutations support real shared identity/classification decisions.
- Reuse measured RM2 portraits/ink fixtures. Simulator text rendering is a
  deterministic visual approximation, not xochitl typography or native writing.
  Real adapter behavior gets a focused hardware regression after refactoring.
- Schema has an explicit product mode. Reader runs; Writer/combined fail clearly
  until REM-23 introduces production behavior. REM-23 and REM-17 must add those
  simulator flows; REM-25 must extend native insertion. Do not fake completion.

## Risks / Trade-offs

Validation discovered that restarting xochitl can merge adjacent mmap allocations
into one anonymous VMA. The previously deployed detector checks only VMA starts and
then reports zero matches. Extend discovery to page-aligned headers within eligible
anonymous writable regions, require the entire expected allocation to fit, and
retain exact 32-bit mmap-header validation plus unique-match refusal. No hardcoded
address, historical offset fallback, heap scan or raw memory dump is introduced.
Test standalone/merged/ambiguous/invalid/truncated mappings and visually inspect
real captures after restart. This is a prerequisite to exercising the real adapter.

- Interface refactor could alter device behavior -> retain real algorithms and
  waits, cross-build and verify one bounded live Reader workflow on RM2.
- Semantic raster output can hide native layout bugs -> exact text/trace assertions
  plus screenshots, document native-layout/keyboard checks requiring hardware.
- Scripted replies cannot assess vision quality -> label them explicitly and keep
  real handwriting/LLM acceptance tests distinct.
- Scenario assertions could be weak -> test preserved source and untouched pages,
  no extra navigation/writes/model calls, and deliberate failing assertions.

## Migration Plan

Existing CLI defaults and service remain real-device execution. Add fixtures and
local launch docs, verify native/ARM and hardware adapter, sync/archive in this PR.
Tablet candidate deploy keeps the existing /home executable with atomic replacement
and rollback; restore service and document state after checks.

## Open Questions

None. Writer/combined boundary confirmed with originating task against roadmap.
