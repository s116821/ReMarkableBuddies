# E0T original dummy-helper preparation

Incomplete preparation only. No device operator or frozen unit/approved packet yet.
Default builds refuse controller/guard; cleanup always refuses. Do not stage or run this
artifact on a tablet. Owning consumer plan is Docs175f22d; SDK02398cf defines the
finite dummy-manager experiment, with exact packet review/approval before execution.

The C worker foundation has fixed roles/cases, compile-time nonce and owned runtime
root. It checks root/file/FIFO ownership, refuses symlink/hardlink record escapes,
and writes disjoint nonce/case/PID/start/monotonic events. At most96 possible record
files x2048bytes =192KiB, leaving64KiB of the proposed evidence budget for bounded
manager/operator evidence. A frozen manifest must enumerate these exact used paths;
this arithmetic is not an implemented total-evidence collector.

Marker/no-restart/no-op workers terminate. Claim spends an exclusive owned record
before intentional failure, then subsequent starts hold a non-failing stock stand-in.
Hold workers self-exit by15s and accept only one-byte X through their owned FIFO.
Notify sends one READY/watchdog datagram using manager-supplied context, then becomes
silent for the dummy watchdog test. No device watchdog, native/Qt/XOVI/data call or
arbitrary executable/command option exists. Internal core/file caps supplement the
future required unit memory/task/runtime limits; those limits are not yet enforced
by a supplied unit. The immutable unit command can read a single owned case byte;
its writer must never advance a case until previous jobs/processes/handlers are
verified terminal. That operator fence is not yet implemented.

Host Linux tests compile with strict warnings and exercise refusal, link protection,
record bound, immutable-command case separation, one-time claim/controlled hold and
notify to an owned fake Unix datagram socket. No live systemd manager runs. Run in a
Linux environment with GCC and Python3:

```sh
python3 tools/dummy_manager/test_helper.py -v
```

The initial vendor-SDK ARM32 hard-float build uses a clearly synthetic placeholder
nonce0123456789abcdef0123456789abcdef. It is not the reviewed transaction artifact.
It imports libc only. Source/target build checks are not actual device load proof.
Next: freeze actor cleanup/restoration/job fencing and complete operator/manifest/
unit/resource table, then compile actual nonce, inspect target imports and obtain
independent source/artifact/operator plus final coordinator approval. No completion,
selection or E1 qualification is claimed.

Corrected worker checkpoint4da6bfc67f452a36aed393d280a16a3ffbd26ed1 received
bounded independent acceptance: seven strict host tests0.778s, original FIFO/lock
reproducers now refuse, and an actual stock stand-in hold exited normally14.051s.
Owner/case/existing-claim/record opens are nonblocking and record locks refuse
contention. A process alarm starts at entry15s; normal hold deadline is14s from
entry. Arbitrarily blocked kernel I/O remains unqualified. Initial94ddc5a was held
for avoidable FIFO/lock waits and is not accepted unchanged.

Preparation-only process/file actors now live in actors.h behind the explicit
E0T_ACTORS host build flag. Default worker builds still refuse controller/guard;
experiment cleanup always refuses pending implementation. These actors use fixed
/proc/self/exe stock children, process start identities, an observed peer death,
and a fresh empty peer cgroup with no descendant directories. The default cgroup
parent matches the read-only tablet hybrid profile; host tests override it with
explicitly FAKE files, so none of those tests qualifies manager/cgroup enforcement.

Restoration and activation publication share a nonblocking owned file lock. Closed
case state permanently fences activation; a one-shot claim is spent before fork.
A missing child identity/receipt after the claim is unknown and never respawns.
Existing completion requires exact receipt/stock identity and current live state.
Late publication attempts actually pass through the state writer and refuse under
the closed fence. Actor-held stock children remain in that actor's unit cgroup;
TasksMax2 and prior start-job/whole-cgroup fencing require future operator proof.
SIGTERM child cleanup uses actual waitpid; arbitrarily blocked kernel I/O remains
unqualified under the entry process alarm and future unit cleanup boundary.

Five author tests use real actors/children with FAKE cgroup files: controller loss,
guard loss/actual late-write refusal, dead cached child/no second claim, nonempty
old cgroup/no fork, and interrupted claim without child publication/unknown. The
actor refinement is coordinated with SDKbb39af1; no dummy-manager job recovery is
inferred from these host tests. Full cleanup/operator/limits/packet review is open.
prepare_packet.py produces local draft files only, marks them runnable=false, and
lists missing gates. Its unit profiles and inode/absence expectations remain drafts;
there is no SSH or stage/start/reload functionality in that generator.

Actor review held initialc6ad6e9 for unprotected activation and an event-only B
barrier. The follow-up writer checks current correlated peer protection and actor
phase under the shared publication lock. B now irreversibly creates the closed-case
fence before reporting restoration-begun; tests inspect that actual artifact before
terminating the actor. C spends the actual one-shot claim without forking, exposing
an interrupted pre-fork phase that the survivor reports unknown. A host-only
E0T_TEST_IDENTITY_FAULT compile variant forces child identity refusal after actual
fork/exec; observed child exit yields unknown with no second spawn. This compile
variant is not part of a device artifact. Eight host actor tests now include fresh
unprotected/unknown-peer refusals and those concrete interruption phases. Cgroups
remain explicitly fake and full operator/cleanup/resource gates remain open.

Candidate resource preparation now applies and reads back kernel per-process limits:
AS8MiB, stack512KiB, data1MiB, file2048bytes, core0 and CPU2s. It sums its own initial
mappings without exporting addresses and requires2MiB headroom (full stack+data caps
plus512KiB reserve). --profile reports actual mapping total and getrlimit values,
with aggregate_kernel_limit=false and device_packet_frozen=false. Each case helper
records verified values. A stock child checks inheritance before fixed self exec.
These are helper candidate limits, not demonstrated ARM loader headroom, command
artifact bounds, resident-memory totals or kernel aggregate task enforcement.

Small host-only E0T_RESOURCE_TEST fixtures demonstrate a refused2MiB allocation
under the1MiB data cap and exact2048-byte kernel file cap/EFBIG. A bad AS sizing
variant refuses before owned effects. E0T_TEST_INHERITANCE_FAULT reduces the child
CPU limit before exec; mismatch yields unknown/no stock exec or replacement. None
of those test-only definitions belongs in a device artifact or extra device case.
The complete packet still needs actual artifact profile/headroom and all command,
cleanup, aggregate output/concurrency and original-service gates/review.

Preparatory E0T_MANAGER transport currently supports only fixed read-only
systemctl --version. It borrows one shared nonblocking slot; any retained intent
or child publication refuses, including after the previous lock owner dies.
The manager parent keeps AS soft8MiB/hard20MiB; its single command child raises
only its own soft limit to20MiB and verifies all inherited limits before exec.
SIGCHLD is reset to default so actual waitpid retains child ownership. Success
requires both observed child exit and output EOF within2s, at most4096 output
bytes, canonical child publication and exact current intent. Overflow/deadline
kills and waits only the held child; missing publication remains unknown.

Five tests compile a small host C fake CLI (never systemctl): fixed argv and
actual child AS limits, serial reuse after wait, retained-claim refusal, slot
contention, overflow and wall-time cleanup. This does not profile the tablet CLI,
qualify writable manager jobs, implement independent cleanup, or freeze a device
packet. The CLI20MiB candidate differs from helper8MiB; aggregate concurrency,
command-specific peaks/headroom and job reconciliation remain open gates.

The barrier worker now accepts one explicit R control byte to send READY=1;
startup alone does not publish readiness. Repeated R refuses, and X remains the
controlled exit. A host datagram test observes no initial message, exact READY
on the first R and refusal/no duplicate on the second. This is worker protocol
only, not proof of queued manager job cancellation or late-start prevention.
The local draft manifest includes the three command-slot metadata paths and
separate helper/manager-parent/CLI AS candidates. It continues to refuse a
runnable/frozen status and requires actual writable-job reconciliation and
command resource/concurrency verification.

jobs.h is a pure preparation-only bounded list-jobs decoder, with no manager
calls or cancellation authority. It accepts at most4096bytes and12owned rows,
requires final newline/exact four ASCII columns/canonical positive uint32 IDs,
exact fixed nonce-unit names, start/stop and waiting/running only, and refuses
duplicate ID/unit, controls/colors/truncation/unknown rows. Caller must discard
ALL rows and count on failure. Empty success is an empty observation, not proof
of completion/cancellation or attribution. Four strict compiled-C host fixture
tests passed0.347s. SDK43b0e220389c62caef23c64c513545b4dc11e7cc recommends
--no-legend --plain --full --no-pager --no-ask-password, fixed known-unit args,
LC_ALL=C and SYSTEMD_COLORS=0, with no --after/--before extensions. Underlying
ListJobs allocates manager-wide data before filtering; resource failure refuses.
No writable dispatch, exact-job recheck/cancel, manager-instance continuity,
late-start fencing or terminal proof is implemented by this decoder.

The preparatory shared slot now supports fixed --manager-jobs as well as version.
It constructs twelve exact compiled-nonce unit arguments with fixed full/nolegend/
plain/nopager/noaskpassword options and execve's an explicit small environment
(localeC, colors0, console/info logging, system executable PATH). No parent env,
preload, shell, arbitrary unit or writable verb is passed. Completed output must
pass the entire pure decoder before publication; diagnostics/truncation refuse.
The same single-slot ownership/actualwait+EOF gates apply. Host fake-CLI tests
verify fixed argv, replacement of hostile inherited log/color variables, absence
of an arbitrary inherited variable, and rejected malformed reply. Full32host
worker/actor/decoder/transport tests passed8.619s. Actual vendor CLI query peak,
resource headroom, manager continuity/job recheck/cleanup and writable actions
remain unqualified; no tablet query or staging was performed for this change.

cancel_args.h is a pure prospective batch argument encoder, not wired into any
manager dispatcher. Zero tuples returns NO COMMAND; at most12distinct positive
canonical uint32 IDs derive from validated owned-role/type/state tuples. Invalid
input returns failure and requires discarding all partial outputs. Caller must
freshly correlate/recheck those tuples against the same manager/case instance
under sole-writer fencing; encoding does not supply that authority or any terminal
cancellation proof. One strict compiled-C fixture verifies zero/max/overflow/null/
duplicate/invalid-role/type/state cases. No cancellation or writable CLI occurs.

The local draft generator now aligns with the corrected authoritative consumer
resource table: at most5programhelper rows +1sharedCLI and2unfrozentransport rows
within8processes/16unverifiedtasks. It no longer writes unavailable MemoryMax/
TasksMax controller values as its resource mechanism. Draft per-process unit
limits use AS8M workers,8M:20M cleanup parent, stack512K/data1M/CPU2/file2048/core0.
These require actual vendor parsing/effective-property/set-get/headroom proofs.
Cleanup draft RuntimeMax210s leaves the original180s cleanup-trigger+30s final
window; its source still refuses cleanup and still lacks role-specific lifetime
implementation. The draft remainsrunnable=false and explicitly lists that gate.

The read-only command budget now starts before lock/intent/fork preparation.
Normal replies must complete by1.5s; the remaining500ms reserves held-child kill,
nonblocking waitpid polling (through1.9s), and bookkeeping/output. A child alarm2s
is independently armed; expiry is never OS-exit proof. If actual wait is missing,
intent/child metadata remain and another command refuses. Final parsing/result
publication checks the2s deadline; publication uses nonblocking stdout then restores
its flags. Uninterruptible kernel/filesystem calls remain honest unknown limits,
not a hard2s wall guarantee. No writable command is added.

E0T_TEST_WAIT_FAULT is a host-only missing-wait-evidence build, excluded from device
artifacts. Its test verifies retained intent/no second fork after deadline and
uses temporary host subreaper ownership to reap only its fixture child. Another
real child closes output while staying alive; EOF is not accepted as completion.
Nine manager tests passed7.193s. The full cleanup/operator budget remains a design
candidate requiring actual accounting and target profiles, not completed proof.

unit_state.h is a pure seven-property observer decoder, not wired into CLI or
cleanup. It requires15distinct complete rows (12compilednonce-owned services and
3fixed original services) with exactId/LoadState/ActiveState/SubState/MainPID/
ControlPID/Job keys once, order independent, no unknown/duplicate/control/truncated
output and4096byte maximum. Original rows have a separate readonly array, not
owned-job/cancel tuples. Jobempty/0 is only observed absence; parsing active/failed/
not-found state does not establish quiescence, terminal jobs or cancellation.

Enumerations derive from exact upstream255.21 unit-def.c load/active/service state
string tables; Job numeric/empty formatting derives from systemctl-show.c. The
seven-field allowed-schema upper bound is15*205=3075bytes including delimiters;
four strict compiled-C synthetic fixture tests passed0.364s. This is NOT proof of
actual absent-unit fields/exitstatus or vendorCLIoutput fit: those remain explicit
pre-integration/freeze gates. No omitted property is fabricated, no arbitrary
nonzero CLI result is accepted, and actual cgroup/process/late-writer proof remains
independent. Caller discards every output on parse failure.

baseline_jobs.h is a pure combined jobs observer, not wired into CLI/cleanup.
At most two frozen read-only baseline unit names remain distinct from twelve
owned nonce roles; baseline tuples use their own type/array and never enter the
owned cancellation codec. Names reject wildcards, slash, whitespace, control/
unterminated strings, duplicates and the Buddy experiment namespace. The full
4096byte observation rejects unknown rows, malformed IDs/types/states, duplicate
units and duplicate IDs across either class. Empty rows remain observations only;
original jobs may naturally finish, so absence is not cancellation attribution.
The exact baseline names/completeness/manager instance come from a future frozen
preflight, not this decoder. More than two baseline units requires packet revision,
not omission. Three strict compiled-C synthetic fixture tests passed0.331s, with
combined row order, maxID and cross-class duplicate/malformed cases. No manager
query, arbitrary mutation target, provenance guarantee or terminal proof is added.

command_args.h is a pure prospective fixed argv encoder, not wired into any
executor. It represents version, owned/combined jobs, seven-field unit observers,
nonempty owned cancel batches, stop11caseunits excludingcleanup, separate cleanup
stop, one exact owned start with job-mode=fail, and daemon-reload. Baseline names
appear ONLY in the combined readonlyquery; original3 names ONLY in unit observers.
No arbitrary verb/unit/string, barecancel or emptybatch dispatch is represented.

Argument encoding conveys no phase/profile/case/namespace/sole-writer/fresh-tuple
or mutation authority. Those are required future caller gates; wrong-phase starts
must refuse before dispatch. Runtime transport remains read-only. The output has
self-referencing pointers and must be consumed in place, not copied by value.
One strict compiled-C canary fixture passed0.124s covering allfixedtargetsets,
empty/maxcancel, full28-argument observer, invalidroles/kinds/unusedinputs, and
readonlybaseline separation. No actual service command, cleanup protocol or device
operation was performed; complete IPC/intent/deadline/resource/operator review
remains open before any writable integration or packet freeze.

Initial pure commandencoder178059e was review-held: an option-looking frozen
baseline name could enter argv as --host=example. The correction rejects all
leading-dash baseline names and inserts an explicit -- end-of-options marker
before EVERY unit/ID target list. Strict compiled-C fixtures now reject --host=,
--help, --all and -x.service and verify literal valid names/delimiters; maximum
argv is29/32. The earlier28-argument description refers to the held revision.
No codec is connected to dispatch and no service/remote command was executed.

request_protocol.h is a pure canonical IPC frame and in-memory intent model;
it is not a durable ledger, IPC server, lease, cleanup actor or CLI dispatcher.
Frames are bounded to64bytes and bind the compiled nonce, generation1..8,
monotonic request ID1..96, fixed opcode and owned role index or batch marker.
Raw job IDs, unit strings and arbitrary argv cannot enter through the frame.
An identical pending request requires reconciliation; a completed duplicate is
historical only and cannot authorize replay. Conflicting reuse or an unknown
outcome closes new starts/advances. Pending mutations, including advancement,
block new starts/advances. Close latches the in-memory fence before completion;
late advancement cannot reopen it. Outcome kinds are checked against operations.

The strict compiled-C fixture passed0.101s including lost acknowledgements,
conflicting duplicates, pending advancement, late completion after close and
malformed frames. These are synthetic model checks only. Integration must
durably publish owned intent before effects and publish the actual fence before
acknowledging close; independent lease, actual process/cgroup exit, fresh manager
observations, phase authority and exclusive takeover remain unimplemented gates.

The read-only transport now consumes the accepted fixed argument codec for
version/owned-jobs and --manager-units. The latter requires all15 complete
seven-property records, keeping original3 services in their separate observer
array. Incomplete/diagnostic/foreign output is discarded before stdout. Any
nonzero CLI status still refuses; actual vendor absent-unit fields/status remain
unqualified. No writable opcode has an entry point despite its pure codec.
Twelve strict host transport fixtures passed8.948s, using an owned C fake CLI;
no real systemctl, manager, tablet or writable action was executed.

intent_file.h publishes only a canonical pending-request frame to an exact
request-001..096 owned path using O_EXCL/no-follow/nonblocking and checked file+
directory fsync. Root/owner token, mode, UID, regular type and single-link checks
gate publication. Identical existing intent returns RECONCILE, never new dispatch;
conflicting, partial, symlink, hardlink or uncertain records remain UNKNOWN and
are retained. There is no deletion/retry, completion journal or ledger recovery.
The strict owned host fixture passed0.221s: actual child exit after publication
leaves intent visible to its parent, with conflict/ownership/partial-file refusal
and exact original bytes preserved. A host-only fsync failure leaves the record
UNKNOWN and retained; subsequent access reconciles rather than republishes.
This is process-loss evidence, not power-loss
or cold-boot persistence on /run, deadline interruptibility, server/lease/fence,
dispatch authority or target qualification. It is not wired into the helper.
The non-runnable draft now enumerates96 pending files (64bytes each),262owned
paths; complete IPC/completion/evidence budgeting and cleanup reserve remain open.

fence_file.h adds a separate owned publication slot and irreversible close file.
Independent file close consumes no request ID. The fenced pending-publication
wrapper serializes close against new start/advance intent publication and refuses
late starts/advances after close; read-only/cleanup intents remain representable.
Held slot, malformed ownership, or uncertain sync refuses. Existing valid close
is re-synced before closure success. Raw intent_file.h remains storage-only and
is not dispatch authority; callers must use the fence and separate phase/model
gates. No actual queued effects, completion recovery, server, lease or CLI is
implemented/wired. Arbitrarily blocked filesystem syscalls remain unqualified.
The strict owned host fixture passed0.196s with actual child close/exit, held-slot
refusal, late publication refusal and close independent of full model capacity.
The non-runnable manifest now264paths including slot0bytes/close32bytes. No
tablets, real manager or services were involved; complete packet gates remain.
