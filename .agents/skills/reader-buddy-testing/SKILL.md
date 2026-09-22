---
name: reader-buddy-testing
description: Test Reader Buddy compatibility and end-to-end Q&A only with full unattended SSH to an authorized development tablet the user is not actively using. This unattended workflow does not apply to personal or actively used tablets, unavailable full SSH, or unrelated edits.
---

# Reader Buddy tablet testing

This document is also a manual checklist for contributors using ordinary Git,
Cargo, SSH/SCP and a screenshot viewer; it does not require Codex, connectors or
private Linear access. Use available public acceptance criteria and discussions.
Without an authorized idle tablet, run the documented local simulator/tests,
record native cases as unverified and let a maintainer complete hardware gates
before merge. Do not request private credentials or imply hardware was tested.

Apply this workflow only when full unattended SSH is available to a development
tablet the user is not actively using, and the task authorizes the proposed tests.
Access alone is not authorization. This workflow does not apply to a personal or
actively used tablet, or when full unattended SSH is unavailable.

Resume from the current branch, saved edits, build artifacts, service state and
evidence before repeating work. A saved successful build or screenshot proves
only its recorded state. Record the source revision or diff and binary checksum
for each hardware test batch; distinguish earlier results from the current build.

## Device and service coordination

- Identify the device and firmware from live state. Authorization for a development
  tablet does not extend to a paired personal tablet. Announce device changes and
  honor the user's existing scope; this skill grants no new mutation permission.
- Isolate test runs from the installed service so two instances cannot inject input.
  Use bounded single-iteration runs and verify startup before injecting the trigger.
  Finish or stop a test process before replacing its binary or manipulating its page.
- Check storage before installation. Stage on a suitable writable partition, retain
  a verified rollback, set executable permissions, then replace atomically. Reload
  service definitions only if changed. Verify the installed binary checksum and
  actual restart behavior; a successful upload alone is not installation evidence.
- Check credentials by presence/permissions or a bounded authorized request, never
  by printing values. Keep keys out of arguments, logs, artifacts and commits.
  Distinguish task-authorized paid API tests from purchasing or redeeming account
  capacity/reset credits; a restriction on the latter does not prohibit the former.
  On missing credentials or a repeated unchanged connectivity failure, finish
  independent checks and report it; do not loop through credential retries.

## Exercise the reading workflow

Use a disposable, authorized test document. For PDF acceptance, use an actual
technical paper with a selected concept and a verifiable question. The circle
selects the topic; surrounding page content and model general knowledge are valid
supporting context. Do not impose a source-only restriction. Paper-specific values
must still match the paper rather than be invented or replaced from memory. Preserve its
original bytes and page order; compare hashes and inspect native annotations.
Prepare a native blank notes page immediately after the source page: the current
app does not insert one automatically.

Validate capture → question recognition → relevant concept explanation → next-page
placement, then a different question appending to the same answer page. Check
the visible question transcription and answer, not just the process exit status.
Use source-specific numbers, uncertainty and units to distinguish reading the
paper from supplying a remembered value. Verify actual rendered operators and
exponents; a correct API response does not prove correct keyboard output.

Include representative connected cursive and shorthand, varying slant and spacing,
plus ambiguous/illegible and absent-question cases. Preserve failures. An abstract
summary after misreading a question is a failure, even if every source fact is true.
The app independently rereads the page without the proposed answer before writing; uncertain
or differing readings should decline the request. Agreement reduces risk but is
not proof of correct recognition. Record conservative false rejections too.
Do not silently replace a failing fixture with easier print or claim universal
handwriting coverage. Synthetic pen/touch events exercise real device integration
but are not evidence of human handwriting or a physical finger gesture.

Check a short tap versus a held trigger, an occupied successor page, and navigation
back to the source. Inspect UI state between dependent actions: selecting an eraser
and opening its menu can require separate taps. Confirm old ink is gone before
drawing another fixture. Keep printed content separate from editable test ink.

## Capture and evidence

The normalized screenshot is useful for navigation but can lose dense PDF digits.
Inspect native detail as well. Full-width overlapping strips preserve entire
question/text lines; left/right crops previously split a readable question.

Save annotated input and resulting output screenshots, the relevant sanitized
log, expected result and actual result together. Record failures and subsequent
repairs without relabeling old evidence as a pass on a newer build. Run code checks
appropriate to changed behavior, and verify the final installed build on hardware.
Do not equate an architecture build with hardware validation on that device.

Use [the hardware probe](../../../examples/hardware_probe.rs) for bounded offline
input actions and [the fixture guide](../../../docs/validation/README.md)
for reusable fixture setup and links to PR evidence. Adapt coordinates to observed UI
state; do not fossilize another machine's paths, addresses or credentials.

Before stopping, restore the agreed service state, leave the test document in a
known state, and remove or stop only test processes created for this work. Report
what passed, what failed or remains untested, the installed build, and the next
concrete step. Firmware changes, pairing/sync, extra providers and new features
remain outside a compatibility test unless separately authorized.
