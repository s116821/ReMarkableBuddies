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
Rollback has a five-second monotonic deadline: blocked rollback explicitly becomes
RecoveryFailed at expiry, and late operations cannot report successful restoration.
Child startup has a two-second bounded hello and cleanup on malformed/absent hello or
missing OS identity. Session construction also cleans up if its own handshake refuses.

The Supervisor creates and holds its injected child before enabling configuration.
The guard independently observes actual Supervisor exit and the shared child's live
OS identity, requests target stop while Supervisor lives or restores stock after it
dies, waits for injected exit, starts a fake stock child
and removes an explicit transaction-file allowlist. Tests independently compare
baseline/unrelated bytes and the restored child's OS process start identity. Windows
identities use GetProcessTimes plus live-process signal state; Linux identities use
/proc stat start ticks and reject zombies. Retained handles of dead children are
not live evidence. These
tests model no systemd/watchdog/Qt/GUI/document-worker behavior.

Guard-alone death now triggers surviving-Supervisor recovery on loss of its private
guard pipe. It restores baseline bytes, waits for the exact injected child to exit,
starts a separately owned stock child and records verified completion. Tests inspect
the actual stock OS identity and files before any runner cleanup. A blocked or late
recovery records RecoveryFailed. Simultaneous protection loss remains a limitation;
Actual death barriers include arming, partial configuration, prepared child before
configuration, applied configuration before stock stop, stock stop and Ready. The
Supervisor owns the child before configuration commit, closing the former apply-to-
identity handoff gap. Restoration disables its serialized activation writer; no old
command can reenable the completed transaction. Atomic temp files are actor-specific.
The surviving Supervisor exposes one transaction-scoped idempotent restoration.
Guard requests and its own channel-loss recovery reuse that receipt/stock process.
Tests issue two requests before reading either reply and lose the restoration reply
pipe: one restoration and one stock identity result. Foreign nonce/process scopes
refuse, as do activation/partial-write requests after restoration. If Supervisor dies,
the independent guard uses its own baseline restoration; no owner lock blocks it.
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
The rollback deadline detects late synchronous filesystem operations; the harness
cannot interrupt an arbitrarily blocked filesystem syscall at exactly five seconds.
The independent 30-second outer watchdog is its final host-process bound. That
limitation and production recovery under blocked I/O remain open qualification gates.

Owning product plan: [Docs native-buddy-page-creation at 1b8ea1b](https://github.com/s116821/ReMarkableBuddiesDocs/tree/1b8ea1b/openspec/changes/native-buddy-page-creation).
SDK contract: [e2b3ebbb](https://github.com/s116821/ReMarkableOpenSDK/tree/e2b3ebbb8c4c630e46041896bc266498518de437/openspec/changes/establish-native-platform-contract).
Source basis: current coordinator E0 authorization, those exact specifications and
original host code. Modeled transitions are assumptions for investigation, not native facts.
