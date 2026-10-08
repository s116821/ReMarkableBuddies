# Experimental shutdown diagnostic packet

Source-only preparation and focused Linux fixtures. Main alone selects the fresh
nonce, compiles the matching SDK recorder, transfers the packet, announces device
changes, and operates the tablet. Nothing here grants native execution authority.

`prepare_packet.py --selection FILE --payload FILE --stock-unit ORIGINAL_UNIT --vendor-dropin ORIGINAL_VENDOR --output NEW_DIRECTORY`
requires a JSON selection containing `nonce`, `budget_seconds` (360), canonical
string `stock_pid`/`stock_start`, `payload_sha256`, `executable_sha256`,
`stock_unit_sha256`, `vendor_dropin_sha256`, exact `original_policy` from the
preparer, and `protected_files` with 24 `{path, sha256}` entries. Main verifies
that this inventory comprises the fresh 14 document and 10 provider baselines.
Preparation verifies bytes and emits a receipt; it does not verify recorder
compile provenance or that the nonce has never been used outside the known list.
Optional `diagnostic_kind` is `lifecycle-only-v1` (also the historical missing-field
default) or `pretoken-facts-entry-v1`; any present unsupported or malformed value
refuses preparation before output creation. The latter selects the separate
`trace-stop-pretoken-proof.awk`, copied to the packet's existing
`trace-stop-proof.awk` filename and bound by its manifest hash. The receipt names
the effective mode and canonical parser source. Main must independently verify
matching SDK mode/source/build provenance; the preparer cannot establish it.
Pretoken admission requires exactly one `entry-installed` after startup with
startup/GUI TID, quit=0, dropped=0, frames=0, followed by qualifying before-render.
Window may precede or follow installation. Missing proof does not admit STOP;
duplicate or invalid installation refuses the snapshot. All existing wire limits
remain fixed. Installation does not prove the SDK hard request fence or Qt teardown.
A prior normal before/after-render makes a later zero-frame installation invalid,
even when another render follows: the actual frame counter cannot reset.
The SDK setup cohort shares the existing root, so actor evidence creation may
stimulate its watcher and queue callbacks. Interpret results as a coarse setup and
timing discriminator; SDK source/fixtures separately prove no owner/getter/reader
admission even for unexpected tokens. Actor, launcher, guard and budget are unchanged.
SDK preserves ordinary observer connection ordering and records the actual frame
counter. If rendering advances before installation, frames=0 eligibility fails
closed and recovery follows; the counter must never be reset to obtain admission.
Original unit/vendor bytes must match their pinned hashes. The packet retains the
originals privately and derives shadows by removing exactly one Unit OnFailure
assignment from each, preserving every other byte. Do not publish private source
captures. Empty dependency assignments in drop-ins do not clear prior OnFailure
dependencies on systemd 255; effective runtime provenance remains a native gate.

## Main review and arming recipe

1. Review exact source, SDK commit/build receipt, selected artifact, preparation
   receipt and every protected path. Refresh original PID/start, unit/vendor and
   executable hashes, policy, service health and absent jobs. Reject any drift.
2. Verify the tablet has `/bin/sh`, `/usr/bin/systemctl`, `flock`, `sha256sum`,
   `stat`, `awk`, `sed`, `cat`, `cp`, `grep`, `sleep`, `kill`, `dd`, `wc`, `tail`,
   `od`, `tr`, `mkdir`, `rm` and `rmdir`. No timeout utility is required.
3. Transfer into the initially absent receipt-selected `/run` root, root-owned
   0700, with all packet files regular, single-link, root-owned 0600. Verify
   `packet.files` against the preparation receipt and its contents remotely.
   The runtime xochitl full unit and drop-in directory must be initially absent.
4. Persist `deadline` exclusively as `armed_at deadline\n`, using integer
   `/proc/uptime` seconds and deadline exactly armed_at+360, mode 0600. Do this
   before starting the actor; never rewrite it or reuse a spent root/nonce.
5. Independently start a unique transient service named for this nonce, invoking
   `/bin/sh ROOT/actor.sh`, with Type=oneshot, TimeoutStartSec=360,
   Restart=no, OnFailure empty, FailureAction=none, StartLimitAction=none,
   KillMode=control-group and no dependency on the host connection. Verify its
   effective properties, owned process, running state and private `actor.ready`
   before requesting any guard or activation change. A disconnect must not stop
   this unit. Do not use an SSH foreground actor or a tied host job.
6. Exclusively write private `guard.request` containing the nonce. Verify actor
   `guard.ready`, the exact owned full unit, same-basename vendor shadow and guard
   drop-in, effective FragmentPath/DropInPaths/ExecStart and OnFailure empty,
   Restart=no and unchanged remaining guard policy. Only then exclusively write
   `start.request`. Requests have a ten-second window each; a missed window is a
   spent attempt, never permission to restart the actor.
7. The actor owns STOP, activation, observation and restoration. Do not issue
   competing systemctl operations. Capture initial/candidate STOP records,
   trace, `diagnostic.outcome`, `diagnostic.failed`, query evidence and either
   `stock.restored` or `recovery.failed`. Preserve failure even when stock is
   healthy. Missing terminal lifecycle records remain unknown.
8. Independently verify original hashes/policy, no owned overrides or payload
   mappings, empty job and all three services healthy. If termination or query
   state is uncertain, retain the guard and evidence for Main's recovery review;
   do not blindly retry, remove policy protection or start competing stock.

## Local verification

`test_actor.py` runs in an owned Linux fixture with no network or actual service
manager. Nineteen checks cover conditional query failure, restoration ordering and
crash preservation, deadline refusal, failed process reads, bounded owned query
children, trace identity/drop/sequence, request expiry and singleton claims/lock.
They also exercise preparation with synthetic bytes, actual actor
entry/recovery shells on missing host requests (including after guard install),
and refusal to verify stock when only xochitl is active.
The primary-failure followed by candidate-crash regression exercises the actual
EXIT handler and separate recovery shell, preserving existing failure evidence
while restoring stock once. Foreign, malformed or linked failure markers refuse.
Only valid failure evidence is idempotent; every physical action claim is exclusive.
Additional cases preserve failed guard snapshots across stock proof, verify exact
shadow derivation/provenance and partial-install cleanup or foreign-content refusal.
Preparation uses synthetic source hashes as an injected input boundary; it does
not claim private native source or live systemd validation.
A minimal-BusyBox-od regression preserves the actual unsupported-option failure
and verifies checked tail/wc LF proof, parser admission and failure-marker retention.
The temporary fixture mount must permit its owned query stub to execute. These
checks do not establish native correctness or recovery after an executing native
candidate loses its host connection.
