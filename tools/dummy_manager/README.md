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
