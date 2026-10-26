# Exact upstream systemd 255.21 parser fixture

## Recovery command entry fixture

`recovery_entry.py /absolute/path/to/reader-buddy` exercises the built application
in a disposable root Linux container with Python 3.11+, compatible binary libraries,
no running systemd, and no preexisting `/run/reader-buddy-resident`. It refuses
other contexts. Mount the binary and this script read-only; give the container
private writable `/run` and `/tmp`, no device mounts and no network. This fixture
must never run on the tablet or against an existing installation.

The bounded subprocess cases verify the exact recovery command before poisoned
configuration/Store/provider startup, no-record no-op output, missing directory,
partial/malformed/symlink/directory/FIFO refusal, protected directory permissions,
mixed-mode rejection and ordinary startup's occupied-latch refusal. It removes
only its own known test leaves. It performs no PID1 recovery, stock activation,
native operation or document check; service and native qualification remain
separate from this command-dispatch coverage.

`health_entry.py /absolute/path/to/reader-buddy` uses the same disposable-container
boundary. A real local Unix socket checks that failed ordinary initialization
cannot emit readiness. Malformed/wrong-owner notification settings refuse before
configuration; Writer, maintenance, recovery and simulator dispatch skip the
notifier. Dotenv cannot introduce notification settings after startup capture.
All subprocess calls are bounded and use poisoned configuration, with no device
or model access. This does not exercise installed PID1 watchdog/recovery behavior.

## Offline unit parser

This original tooling builds upstream systemd-stable tag v255.21, peeled commit
70500d37992a01d3275b1c414c3ed161d6f91f9e, and exercises its offline test mode.
It uses disposable fake units, an unprivileged parser, empty generator paths,
and no tablet, host mounts, device mappings, or external network at run time.
No unit or job executes. The tablet vendor patch set is unknown; matching the
reported version does not establish identical vendor behavior.

From the repository root:

```sh
docker build -t rem25-systemd-25521:host-fixture tools/systemd_fixture
docker run --rm --network none --cgroupns private rem25-systemd-25521:host-fixture python3 /opt/buddy/parser_fixture.py
```

Build requires network access and Ubuntu package repositories. The upstream
source commit is pinned; the Ubuntu image and package versions are not pinned,
so this is source reproducibility, not a byte-reproducible build. The report
records compiler and installed systemd executable SHA-256 for each run. Systemd
retains its upstream licenses in /opt/systemd-source; no upstream source or
binary is committed here. These scripts follow this repository's license.

The assertions cover:

- Empty OnFailure followed by an owned handler preserves the vendor handler,
  while Restart=no takes effect.
- A runtime full fragment still inherits the vendor unit-specific drop-in.
- A separate named unit has no handler when none is configured.
- A separate named unit inherits a generic service.d handler.

Each scenario verifies the vendor fragment remains byte-identical. Dump hashes
are evidence identifiers, not deterministic golden values: process metadata in
systemd dumps may vary.

The October 2 host run passed all four scenarios using GCC 13.3.0 and executable
SHA-256 f132ac4469673ce3aead14c70b286ebfab2d0e22f572d25d95040b7b7f54f4be.
Image SHA-256 was 22f2673ae3341f7ffde895aa9bcf89e9fa3d745829107239d45569c2005395cd.
The full report is retained in the coordinator's outputs directory.

Live-manager experiments could not allocate their own manager cgroup on this
Docker host's read-only cgroup v1 hierarchy. Scoped system-manager and private
user-manager attempts failed and their owned containers were removed. No host
cgroups were modified. This fixture therefore does not qualify job cancellation,
stop deadlines, failure-handler execution, service coupling, watchdog recovery,
or any tablet activation. E1 remains unapproved.

## Isolated live recovery lifecycle fixture

`recovery_lifecycle.sh` is a separate, explicitly invoked development-host test.
It creates three fresh runtime units: fake stock, an actor with an owned sleeping
child, and an independent recovery helper. It never starts/stops real xochitl or
reader-buddy. Run only on an authorized idle development tablet after announcing
the temporary service changes. The present fixture requires the observed hybrid
cgroup2 mount at `/sys/fs/cgroup/unified`; it refuses other layouts.

```sh
sh recovery_lifecycle.sh FRESH_32_LOWERCASE_HEX_NONCE clean
```

Replace the placeholder with a newly generated nonce for each run. Other cases
are `crash`, `watchdog`, `stop`, and `recovery-retry`. Never reuse a run directory
or unit name. `populated-delete` additionally attempts one nonrecursive removal
of only the actor's exact group while the actor and its child belong to it. It
requires the busy diagnostic and unchanged group identity/population before
normal exit and recovery. This establishes direct populated-group refusal only;
it does not test a child-only nested group or privileged migration.
All three units have independent runtime/start/stop bounds. The
driver verifies distinct recovery invocations and exactly two fake-stock starts
(initial plus restoration), including a helper failure before restoration and a
second restoration request after success. Cleanup stops only its own units,
verifies inactivity/jobs/group state, and removes their definitions only after
that check. Failed cleanup returns failure and preserves definitions. Retain and
archive `/run/buddy-recovery-NONCE` before removing that exact evidence directory.

The script brackets the experiment with actual stock/Buddy process and invocation
identities plus stock artifact hashes. Cgroup disappearance is recorded explicitly
as an observation; it is not accepted as a production emptiness proof. This fixture
also does not test real shutdown cancellation, native loading, UI readiness,
document safety or the installed product's recovery implementation.

On October 25, all five cases passed on the authorized RM2, firmware 3.28.0.172,
kernel 5.4.70-v1.6.3-rm11x, systemd 255.21. The exact script SHA256 was
`6be67aa2672c0760aba0d8dc4a7f0e774473c6b070a5fc65ec711acc73b055df`.
All owned units were stopped and removed; real stock/Buddy identities and stock
hashes remained identical. PID1 removed the actor's group before every helper
invocation. Detailed evidence and remaining product gates live in the coordinated
Docs change `qualify-open-pdf-capture/resident-supervisor-fixture-result.md`.

The current script also samples the actor's InvocationID, ExecMainPID and
ExecMainStartTimestampMonotonic before fake-stock stop and at each recovery call.
A separate fresh clean run showed these fields retained on the first recovery
and fully cleared on the second. Recovery cannot assume PID1 retains the original
incarnation indefinitely. The coordinated design defines the additional original
lease, deleted-group and activation-latch checks required for that case; this
fixture does not implement or authorize production recovery.

## Bootstrap controller fixture

`bootstrap_fixture.py` exercises the built development-only
`resident_bootstrap_control` in fixture mode, using four nonce-owned runtime
units and disposable source data. It requires Python on the coordinating host,
SSH/root access to an already authorized idle development target, and the
observed hybrid cgroup mount. It does not require Python on the tablet. Announce
the temporary service changes before executing it. Source review and a matching
compiled artifact are prerequisites; this script grants no execution permission.

Create a protected local JSON file containing the authorized SSH executable,
options and target as an argument array, without a remote command or password.
Use the actual build-source SHA from the diagnostic artifact's `source.sha`.

```sh
python tools/systemd_fixture/bootstrap_fixture.py \
  --ssh-prefix-file /local/ssh-prefix.json \
  --binary /local/resident_bootstrap_control \
  --source-sha EXACT_40_CHARACTER_BUILD_SHA \
  --case none --output /local/evidence
```

Other cases are `actor_kill`, `actor_hang`, `after_buddy_stop`, `recovery_exit`,
`recovery_after_intent` and `recovery_after_start`. Each invocation selects a
fresh nonce, captures real stock/Buddy identities and artifact hashes, transfers
the controller exclusively, checks its checksum, freezes the fixture candidate,
and runs one actor followed by exactly one explicitly selected second helper.
An uncertain stock-start intent must prevent another start even across helper
invocations. Actor crash/timeout cases require an observed live child in the
actor's actual group before the injected fault. Assertions distinguish successful
restoration, unchanged unadmitted stock and intentional uncertain-start refusal.

The runner preserves every command selection, output, candidate and control
archive, including failures. Never replay a failed or timed-out actor. First
inspect its live PID1 state and retained evidence; a host observation timeout is
not proof that the remote unit stopped. All fixture services have independent
PID1 limits. They address no real stock/Buddy unit and load no native adapter.
Source-mismatch and shutdown cases remain outside this driver's coverage.

Cleanup is deliberately separate: after collecting and verifying the exact
nonce's evidence, confirm actor and helper terminal with no job, stop only the
two owned fake services, verify all four units have no process/job/population,
then remove only their exact runtime definitions and reload PID1. Preserve
payloads and evidence until separately verified collection. Do not recursively
remove an assumed or computed parent directory.

Offline verifier tests run without SSH or PID1:

```sh
python -m unittest discover -s tools/systemd_fixture -p test_bootstrap_fixture.py -v
```
