# Prepared one-fact memory forget

This protocol adds a deliberate bounded maintenance path. Inspection-only v0.17.0
and older builds do not support it; unknown selectors/dash refusal are unsupported,
not permission to stage a remote file or call a running Buddy service. Existing
absolute-file Forget remains compatible and has its older DTO/receipt behavior.

The full versioned wire, canonical hash recipe, golden vector and phase/status
matrix are in the central [prepared-forget contract](https://github.com/s116821/ReMarkableBuddiesDocs/blob/64ff69baf2e066f203eadd587cb2d3864173b1a6/openspec/changes/prepare-conditional-memory-forget/contract.md).
No component-local OpenSpec copy is required.

## Three separate operations

Run one fixed command with an explicit absolute config, supply one UTF8 JSON
request on stdin, close stdin and consume one report-v1:

```text
reader-buddy --memory-operation prepare-forget --memory-input - --config ABSOLUTE_CONFIG
reader-buddy --memory-operation forget --memory-input - --config ABSOLUTE_CONFIG
reader-buddy --memory-operation forget-receipt --memory-input - --config ABSOLUTE_CONFIG
```

`prepare-forget` takes `{prepared_version:1,operation:UUID,key:KEY}`. Generate one
non-nil intent UUID for this deliberate action. The strict Key is the same
configured-profile identity as inspection, with global/document nested unknown
and duplicate fields refused. Success returns `{schema_version:1,preview:ENVELOPE,
intent:PREPARED_INTENT}` under report data. The preview and conditional evidence
come from the same admitted view, sealed through the consuming writer. Only one
canonical supported current value qualifies; absent/tombstone/conflict/opaque
states refuse. Automatic memory may be disabled, but a profile and existing owned
Store are required. Observed/expiry values above9007199254740991 refuse this
preview without narrowing the older domain/file APIs.

`forget` stdin takes only `{intent:PREPARED_INTENT}` after explicit confirmation.
The privileged host retains the original packet privately with its verified
physical target/boot/session context; a renderer cannot replace Key/heads/paths/
mode/pins behind a displayed confirmation. Search or ordinary inspection is not
a prepared intent. Stdin is inclusive1048576 bytes with EOF, and encoded response
is at most8454144 bytes including newline. Caller deadlines remain caller-owned.
Put/setup selectors continue early unsupported without reading stdin. The two
new selectors require stdin; existing file operations keep their behavior.

`forget-receipt` takes the same original intent and is read-only reconciliation.
It never calls confirmation/publication when no matching receipt can be proved.
Do not automatically retry or create replacement intent after lost output.

## What is bound

Preparation captures the exact Key, original operation UUID, all current heads
(initially one), each typed Envelope content digest, exact nonsecret config
binding, original mode token and Store actor/generation. Shared selection also
captures full group/profile/binding identity and catalog/all-aggregate acceptance
scope, generations, accepted base, typed selection digest and original raw accepted
head digest. Persisted evidence is not a live SelectionToken or path capability.

Confirmation uses the original mode token. Mode switch/re-entry or config/path/
profile/namespace/accepted-selection replacement refuses rather than refreshing
preconditions. Local first publication holds one Store mutex continuously across
original generation/content/head checks and publication; an ordinary lease alone
does not prevent restore. Selected publication keeps existing admission gates and
fresh-token CAS. Process/mode/provider ownership and output seals remain live
through the actual single writer. Missing storage is not initialized.

## Receipts and uncertainty

Successful confirm/receipt data is exactly `{schema_version:1,outcome:"acknowledged",
operation:UUID,transaction:UUID}`. It acknowledges a historical original deletion
request. A later value may exist; this is never a current-deleted claim or a new
mutation capability. Exact replay can return that original receipt without a
second tombstone. Each expected content digest must match the accepted original
parent Envelope, not merely its revision UUID. Selected acknowledgement additionally
matches the original direct predecessor, with original current config/mode/catalog
and non-target pins conservatively required. Those current admission checks do
not prove a historical full-packet fingerprint; none is stored.

Before any attempt, a validated complete refusal may say not-dispatched. Once
mutation may have occurred, failure reports unknown; missing, partial, nonzero,
late or unparseable output after possible dispatch is unknown regardless of a
closed channel. Receipt failure says unresolved and is not proof nothing committed.
A prefix write failure never appends another report or replays an operation.

Clear stale detail after dispatch. Keep the original UUID/packet; do not silently
replace heads/mode or optimistically remove a fact. Receipt-only lookup must be
deliberate. Fresh deliberate inspection is required for current heads, and even
that is not proof an interrupted earlier operation never committed. Initial
Manager custody is bounded to a live host; losing its private packet on full
restart does not turn an unknown outcome into not-dispatched. Durable recovery
is a separate capability, not implied here.

Forgetting writes a tombstone and preserves history and original documents. It
reclaims no disk space. Selected local publication does not promise immediate
cloud propagation or start a provider, worker, service, native action or account
sync. All operations use existing owned-storage/process admission; normal Store
opening can perform its existing recovery and is not filesystem immutability.
