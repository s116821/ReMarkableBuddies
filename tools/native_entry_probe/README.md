# Development Qt entry and existing-engine QML experiments

Concrete one-attempt development packets, separate from the deferred E0T framework.
The earlier SDK `2265ebb2f993e8806a77d94a352a7fd0de2c3e42` payload proved nonce and
Qt application-thread entry on hardware. SDK `e017ccc` / Buddy `6074e56` then ran
once with nonce `5ce20d48f7c54640b073eea27f27f948`: existing-engine/application-thread
acquisition succeeded, but helper compilation refused with component-error and
helper/controller availability false. Original restoration and exact cleanup
passed; source, payload and private receipts remain preserved. Do not retry it.

The distinct refinement pins SDK source
`0149b5d14cc83e6e905643cd1e952b621e58adc4`, with versionless library import matching
the pinned community pattern and bounded fixed compilation-error categories.
The earlier failure cause is unknown. Main independently reviewed this source
and reran all fifteen owned firmware-matched ARM-emulated fixtures successfully.
Nonce is `f4851e410f62491da7302dd86c5a5b19`; payload SHA256 is
`878bcfae923ef4fc2a50d5622a6f2983463d0897cca72fde6ea939ecb34288bd`.
Exact artifact/operator review is required before execution. It uses application/window/component readiness
events and requires one unique existing engine on the application thread. A fixed
QML helper checks its own DocumentController availability boolean; no controller
methods, page operations, navigation or new engine belong in this experiment.
Imports and singleton resolution can invoke registration code, change engine
association/ownership and evaluate bindings. This is not a pure read-only guard.
Weak guards do not pin native lifetime or identify a displayed source. Source,
firmware-matched fixtures, payload and exact operator review precede the one run.
Emulator fixtures are not hardware evidence. The private typed guard packet is
preserved in history and private evidence, unexecuted and no longer the next path.

`-PrepareOnly` creates the exact LF staging packet locally without contacting the
tablet. Ordinary independent review of the source and generated packet is
required before the one hardware run. The operator pins the current original
process identity, firmware, executable, QtCore/Qml/Gui and vendor unit files. A changed
baseline refuses activation. Evidence and payload remain private.
The public route needs no private metadata address profile. Both original and
attempted generations retain executable and process-identity verification.

The runtime-only stock-name drop-in retains the original restart policy. An
exclusive claim is consumed before the single preload exec; subsequent starts
use stock. Before installing that drop-in, an existing-systemd transient timer
arms one rollback service. Main and timer start that same oneshot service;
`RemainAfterExit` prevents a second successful execution. A failed execution's
claim permits fresh stock verification only, never a blind second restart.

The timer requests rollback after 45 seconds; stock stop/start may each take
90 seconds. This is not a 45-second recovery guarantee or production recovery
qualification. A fresh healthy-stock observation includes original file hashes,
original command and restart policy, stable MainPID/start, active original three
services, and a bounded stock-cgroup check for the payload mapping. Only then
does the operator collect evidence, stop its units, and remove exact owned paths.
Uncertainty retains staging and the receipt. No automatic experiment retry.

This short experiment assumes the root-owned runtime namespace and systemd
manager remain unchanged by external actors during the attempt. No manager
reexecution, reboot, concurrent configuration writer, firmware change, or account
operation is part of the experiment. Recovery uncertainty requires inspection;
it is not authorization for another payload start. The deferred E0T work remains
unfinished and must be reassessed before an unattended production claim.

The actual first packet refused before entry because target BusyBox `flock`
lacked the assumed `-w` option. The corrected fresh-nonce packet proved the Qt
callback on hardware. Its first restore invocation exited 1 for an unknown
predicate; the original operator failure flags remain preserved. The timer's
later spent-claim invocation verified stock without repeating the physical
restart, and subsequent fresh verification/exact cleanup were recorded separately.

The next operator revision records a fixed stage/exit receipt on restoration
failure and the first refused stock observation. After the sole physical restore,
it makes at most ten verification subprocess observations within a 20-second
monotonic admission window, checking time before and after each observation.
Verification may write only its small owned diagnostic; it never changes original
services. It cannot guarantee an individual kernel/manager call returns within
that window. The existing rollback service's 240-second timeout covers its whole
cgroup; an overrun remains failure, not a late success. No `timeout` command or
BusyBox timeout applet exists on this tablet, and none is installed for this test.
