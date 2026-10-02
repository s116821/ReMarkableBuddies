# E0 host-only lifecycle experiment

This is original experimental Buddy policy tooling, not a production Supervisor,
SDK adapter, XOVI payload, installer or device activation mechanism. It never runs
tablet/service commands, accesses a device or uses network connections.

Run from the Buddy checkout with ordinary Python 3.11 or newer:

```powershell
python -m unittest discover -s tools/lifecycle_harness -p 'test_*.py' -v
```

The deterministic policy model uses an injected monotonic clock. Real-process cases
run a fake independent guard, Supervisor and stock/injected children in unpredictable
token-owned temporary directories. Every test case has a 30-second outer watchdog;
real readiness and heartbeat bounds are 5 and 3 seconds. Explicit pipe events sequence
state barriers. No wait duration establishes readiness or successful recovery.

The guard independently observes its actual Supervisor child exit, kills and waits
for its owned injected child, restores exact baseline bytes, starts a fake stock child
and removes an explicit transaction-file allowlist. Tests independently compare
baseline/unrelated bytes and the restored child's OS process start identity. Windows
identities use GetProcessTimes; Linux identities use /proc stat start ticks. These
tests model no systemd/watchdog/Qt/GUI/document-worker behavior.

Guard-alone death deliberately leaves an **unprotected failure**, with Supervisor
still alive: it is not successful automatic rollback. The runner then stops owned
orphan fixtures as cleanup. Simultaneous protection loss is likewise a limitation;
simulated cold-boot reconstruction resets fake runtime-only state and preserves an
inert payload. That reset is an explicit host model, not evidence of real boot recovery.
Interrupted rollback preserves injected configuration and reports RecoveryFailed;
it cannot claim completion. Production selection requires recovery beyond these
current limitations.

The broad E0 matrix also includes deterministic stale scopes, guard unavailability,
duplicate requests and management interruption. Real cases supplement those model
checks; they do not exhaust arbitrary scheduling races or partial filesystem failures.
The current harness has synchronous small fake-child acknowledgments guarded by its
outer watchdog, not a production bounded RPC implementation. No host pass authorizes
E1. No live source continuity, native mutation, licensing or tablet timing is qualified.

Owning product plan: [Docs native-buddy-page-creation at 1b8ea1b](https://github.com/s116821/ReMarkableBuddiesDocs/tree/1b8ea1b/openspec/changes/native-buddy-page-creation).
SDK contract: [e2b3ebbb](https://github.com/s116821/ReMarkableOpenSDK/tree/e2b3ebbb8c4c630e46041896bc266498518de437/openspec/changes/establish-native-platform-contract).
Source basis: current coordinator E0 authorization, those exact specifications and
original host code. Modeled transitions are assumptions for investigation, not native facts.
