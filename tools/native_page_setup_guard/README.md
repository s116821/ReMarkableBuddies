# Positive development setup guards

Source-only tooling; no current or spent packet is changed. Future integration
must stage/hash/clean all selected scripts and obtain independent acceptance.
The shared admission module factors the accepted arming checks; both the guarded
command and future arm consumer use it. Missing stage, waiting evidence, or live
generation refuses admission, even when closure/callback files are also absent.

Main explicitly selects one absolute executable plus literal arguments for each
effect or capture. `guard-one-command.sh NONCE SECONDS EXECUTABLE ARG...` permits
one command bounded to a caller-selected 1–5 seconds plus at most one second for
forced termination. The existing admission lock spans validation and that command;
it never spans download, image inspection, or a human wait. A nonzero/timeout result
is uncertain and stops setup; it grants no retry. Preflight the exact target timeout
flags and termination behavior on an owned harmless fixture before future staging.
An effect and a separately guarded capture may share a transport invocation, but
they remain separately explicit commands, each with fresh admission checks.

After source capture is downloaded and visually verified, the separately invoked
`arm-after-source.sh NONCE` rechecks shared admission and retains accepted atomic
no-clobber publication. It does not verify the image or grant native owner authority.
No caller should invoke it merely because a capture command returned successfully.
The exact future packet must include the existing owned `ln -T` capability preflight.

Dot-source `time-explicit-step.ps1` on the local host and invoke the timing function
once around an explicitly selected effect/capture/transfer/review/arm operation.
Use a fresh receipt path; receipts record monotonic start/end/duration and exception
or returned status. Nonzero native exit throws. Local timestamps support analysis;
they cannot extend remote SDK deadlines or prove native owner/source validity.
Never wrap an interactive wait in a remote admission lock.

SDK setup20s, native access5s, operator observation35s and rollback45s are unchanged.
The timeout here bounds a single process, not a new setup window. Fixed20s feasibility
remains unverified. This tooling adds no automatic navigation, server, polling,
retry, packet nonce, build, or device operation. Original operator recovery remains
mandatory on cancellation and uncertainty. Tests model source admission and process
bounds only; they do not simulate UI effects or native ownership.
