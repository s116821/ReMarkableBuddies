# Caller-selected insertion recovery (source only)

## Open-document source utilities (not executable integration)

The new `open-document-insertion-v1` partition validator protects the exact
current seven-page order and 26 paths: sixteen document files (fourteen immutable,
two mutable) plus ten providers. It includes the intentionally stroked 5aef page
and thumbnail. `prepare_packet.main` and the coordinator still reject this mode;
no new packet can run through these utilities. The legacy six-page mode retains
its existing admission and behavior.

`open_document_timing.py` is a pure proposed timing prerequisite, not wired into
the actor. It preserves the original 360-second absolute deadline and 185-second
recovery reserve, requires 255 seconds before candidate start, and caps observation
at 70 seconds. Stages expire at candidate +20/+35/+40/+55/+60 seconds (ready,
ordinary open/verification, trigger, result, stop), leaving ten seconds of margin.
Creation additionally needs at least 215 seconds until the absolute deadline and
a verified candidate-bound SDK gate expiry. Unknown SDK expiry refuses. Neither
waiting-file mtime nor a readiness/token event renews any limit.

Actual prior setup was 27 seconds, Library readiness 12.608 seconds, observation
timeout exactly 30 seconds after candidate start, and recovery 29 seconds later.
Those measurements support conditional feasibility, not a guarantee of finishing
ordinary opening or validation inside these caps. The future bounded host sequence
must decide without chat, prove its declared observations, intersect SDK and host
expiry, and refuse/restore on uncertainty. Runtime gate/host/actor integration,
focused independent review, artifact/current-state/budget gates, and actual hardware
selection remain pending. These pure tests never open SSH or touch a device.

This distinct experiment uses the original creation Probe and optional sixth
caller-selected UUID argument. It does not invoke shutdown/FactsEntry/engine-ready
diagnostics. No target artifact, nonce, packet or device execution is selected by
these files. Original native_shutdown_probe sources and spent packets are unchanged.

The actor derives its fixed service ownership, one-attempt claims, 360-second
absolute deadline, independent recovery child, process/cgroup checks, guarded
stop, owned cleanup and verified stock start from the existing consumer actor.
The host arm/coordinator derives its bounded independent systemd-run admission
from Main's reviewed private R5 source. These shared safeguards are not evidence
that this new insertion has been run or that shutdown is qualified.

## Reviewable changes

Admission retains all 24 fresh baseline hashes: 14 fixture document files and ten
exact runtime provider paths. The PDF, three existing ink files, local/pagedata and
six thumbnails remain declared immutable. Only the fixture's exact .content and
.metadata paths are expected mutable. The two declarations partition the entire
baseline and are themselves bound by the packet manifest. Backup, target absence,
six forward/reverse page IDs, source/artifact binding and later exact semantic
comparison remain mandatory Main gates; the preparer cannot prove live freshness.

Recovery validates packet/runtime/service safety before stock start. It never
requires document prehash equality to restore stock. After stock is verified, it
records immutable-document comparison and marks insertion.failed on damage.
Mutable prehash differences are observations only, not successful insertion.
Unknown identity/order/new files/damage remain failed or indeterminate outcomes.
The actor never rolls back user files and never retries insertion or service start.

After bounded creation callback/diagnostic observation, Main may write exactly
one private 33-byte stop.request containing the selected nonce and LF. This is a
request to stop, not a native success claim. The actor verifies request ownership,
size and current attempt identity. Missing request, host loss or dead candidate
falls through the unchanged bounded observation window into recovery. Missing
launch identity does not invent a PID: the verified owned guarded unit is stopped
and its whole cgroup must be gone. Result-storage failure does not skip recovery.

The remote root is /run/rmb-qt-probe-<fresh nonce>, matching the unchanged SDK
record() output root. It must never reuse the spent 8287 or other prior roots.
The new guard prefix is zz-rmb-insertion-; coordinator rejects diagnostic receipts.
Default coordinator mode renders only. No transport occurs without --execute-main.
The original independent arm-before-guard sequence remains, followed by bounded
read-only monitoring. Main's eventual stop request is separately selected once.

## Focused verification

Ten owned actor/schema Linux unittest methods pass: expected metadata mutation,
damaged original ink, runtime-safety refusal, missing launch identity, failed
primary evidence storage, abnormal candidate exit with failed evidence marker, complete synthetic 24-path partition/refusals, and exact/stale/missing/replayed stop-request handling. Nine insertion coordinator fixtures also pass, including bounded actor/guard admission, pinned USB transport, packet tamper refusal and default render-only behavior.
They mock service/process observations and do not prove hardware or lifecycle.
No historical private packet is required by these reusable fixtures.

Run in the existing Linux fixture image with network disabled and read-only source:
`python3 -B tools/native_insertion_probe/test_actor.py -v`.

Pending: independent exact source review;
fresh target build/config/artifact review; fresh hardware identity/full backup;
Main's explicit one-run selection and subsequent semantic/visual/restoration
readback. Native authority, crash durability and product qualification remain open.
