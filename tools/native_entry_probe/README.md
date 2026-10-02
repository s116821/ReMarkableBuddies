# Development Qt startup experiment

One frozen development-tablet experiment, separate from the deferred E0T
framework. The payload proves only its nonce and execution on the Qt application
thread. It performs no controller lookup or page mutation. The payload source is
SDK commit `2265ebb2f993e8806a77d94a352a7fd0de2c3e42`; the operator pins its ARM
artifact SHA-256. Its host fixture does not prove delivery on the tablet.

`-PrepareOnly` creates the exact LF staging packet locally without contacting the
tablet. Ordinary independent review of the source and generated packet is
required before the one hardware run. The operator pins the current original
process identity, firmware, executable, QtCore and vendor unit files. A changed
baseline refuses activation. Evidence and payload remain private.

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
