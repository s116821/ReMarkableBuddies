# Inspect one memory fact over stdin

This is the bounded read-only stdin transport over the existing fact inspection API.
It does not add mutation, setup, Reader/model or native authority.

## Invocation and input

```text
reader-buddy --memory-operation inspect --memory-input -
```

Send one UTF8 JSON document, then EOF. The document is at most1048576 bytes,
including whitespace; the reader consumes at most1048577 bytes to detect excess.
Caller owns child/channel deadline and cancellation. Input is a Key, not a wrapper:

```json
{"profile":"11111111-1111-4111-8111-111111111111","scope":{"kind":"global"},"fact":"22222222-2222-4222-8222-222222222222"}
```

An existing document key uses {kind:"document",document:NONNIL_UUID} instead.
All IDs are nonnil UUIDs; profile must equal the configured authoritative profile.
The initial companion uses an explicitly selected global search key. No profile
substitution, current-page inference, operation/expected/write token or arbitrary
remote file path is accepted. Strict new stdin decoding rejects duplicate/unknown
outer and nested fields, including extras on global scope. Existing file decoding
is unchanged and historically ignores extra fields on its unit global variant.

Existing configuration requires an explicit profile, absolute owned roots and
disabled legacy sync. Local memory admits absent/LocalOnly mode; selected memory
requires matching configured SharedGroup group/binding and current registered
selection. Missing config/profile can refuse before reading stdin. Invalid input
refuses before Store admission; no read initializes missing storage. Normal valid
Store opening may perform existing owned recovery. Domain-read-only is not a
promise of filesystem immutability. Put/setup selectors still refuse dash
before configuration, input reading or Store access.
[Prepared forget](memory-prepared-forget.md) is a separate strict confirmation
protocol; this read report alone supplies no mutation authority.

## Report

Report version remains1 and the main path calls existing guarded write_to once.
One UTF8 JSON line, at most8454144 bytes including escaping/newline, has exactly
report_version, operation, status, incomplete and data. There is no query wrapper,
query_status, activation field, live selection pin or private CAS token. Example:

```json
{"report_version":1,"operation":"inspect","status":"ok","incomplete":false,"data":[]}
```

Only ok/incomplete:false with an array is a complete inspection of the requested
key under the admitted view. [] means no current heads for that key in that view;
it is not a complete global inventory, proof the fact never existed, or evidence
of no orphan/historical records. Search is incomplete discovery and may suppress
conflicts, tombstones or expired facts. Inspect returns heads without search
ranking or expiry filtering. Local inspection preserves all concurrent
values/tombstones. Selected inspection retains stricter existing whole-view domain
projection: a conflicted selected head or unsupported/foreign fact/custody refuses
the view, even if another requested fact is individually readable. It does not
return a partial/conflict array, choose a winner or fall back to local memory.

Generic statuses remain ok/partial/busy/unavailable/unsupported/conflict/
invalid-input/failed. Do not reinterpret generic non-ok statuses or alternate data
shapes as a valid inspect array. Missing config/profile/Store, Busy and early
unsupported/input refusals ordinarily carry data:null and incomplete:false;
unavailable after domain/read/guard/cap failure carries data:null and
incomplete:true. Preserve the actual flag; never derive it from the status alone.
Partial/conflict are not a partial successful array or a reason to discard heads.

A smaller consumer cap is allowed but must fail closed on overflow, partial/non-
JSON output, nonzero exit, unsupported version/operation or invalid raw shapes.
No truncation, empty-success substitution, arbitrary staging file or alternate
service protocol is allowed. Older query-only stdin builds return unsupported
for inspect before reading; older file-only builds can reject dash as unsafe input
or lack the operation. This is an unsupported/unavailable capability, not [] success.

## Raw current head envelopes

data is the unchanged array of storage Envelope objects, not decoded Fact values:

- envelope_version:1; namespace:"subject-memory"; domain_schema_version:1.
- record_id, revision_id, operation_id and actor_id are nonnil UUIDs.
- parents is a set serialized as an array of nonnil revision UUIDs; at most4096,
  and it cannot contain the envelope's own revision ID.
- kind is value or tombstone; payload and media_descriptors follow below.

No array order selects a winner. Preserve all distinct current revision IDs and
every head. Multiple heads represent conflict; do not select the last timestamp,
highest confidence or first element. A tombstone is a head with kind:tombstone,
payload:null and empty media_descriptors. It has no payload Key to compare.
Current deletion does not erase historical data or authorize native deletion.

For supported value heads, media_descriptors is empty and payload is the existing
Fact: key, claim, value, subject, tags, provenance, confidence_basis_points,
observed_ms and optional/null expires_ms. Claim is canonical nonblank ASCII-lowered
ASCII-whitespace-normalized text <=128 UTF8 bytes without control characters. Value is nonblank <=4096 bytes,
subject is nonblank <=128 bytes, up to16 nonblank tags <=64 bytes each; text has no NUL.
Provenance has basis:user-confirmed|source-assertion|inferred, nonblank description
<=512 bytes, and nullable conversation/turn/source UUIDs; nonnull IDs are nonnil
and a turn requires conversation. Confidence is an integer0..10000; timestamps
are unsigned64-bit integers and a present expiry is strictly after observed_ms.
Provenance is author asserted, not source/native verification.

Local inspection preserves raw heads, including opaque unsupported payloads;
an ok local report is not proof every Value passed Fact decoding. Selected
projection validates domain facts/ownership/lineage and can refuse the entire
inspection, including for unrelated invalid registered state. A consumer must reject
unsupported envelope/domain/payload shapes as an unsupported view, rather than
silently filter them and collapse a conflict or manufacture an empty result.
Consumers with narrower numeric/string bounds must refuse unsupported data rather
than round, truncate or rewrite it.

## Requested identity and bounds

record_id is NOT fact UUID. Compute the existing Key::record_id: UUIDv5 using URL
namespace6ba7b811-9dad-11d1-80b4-00c04fd430c8 and UTF8 name
"urn:remarkable-buddies:subject-memory:v1:" followed by compact serialized canonical
Key with field order profile,scope,fact, normalized UUIDs; scope field order is
kind then document if present. The global example above derives
aa0b8ad5-2bb5-5515-84d6-668fb768d199. Verify EVERY head's record_id against the
requested key, including null-payload tombstones. Value payload keys must also
match that exact profile/scope/fact. actor_id is writer metadata, not the profile.

Local inspection refuses more than4096 matching heads or more than8388608 encoded
record bytes; each envelope has its existing1048576-byte bound. Selected reads
retain their shared charged budget (64 aggregates,4096 work items,8MiB metadata
and32MiB media) and whole-view validation. Output's separate8454144-byte cap
also applies. Bound/refusal must not return a truncated set as complete heads.

## Currentness and authority

The ordinary one-shot retains Store/process, mode and provider lifetimes through
stdout. Selected Inspection additionally retains sealed current catalog/aggregate
pins and revalidates under existing admission at the consuming write. Replacement,
forgery or stale pins before any output yield the existing unavailable report.
Once a write attempts a prefix and fails, no replacement JSON is appended. This
does not grant continuing freshness after receipt: clear stale detail on host/
connection/boot/request generation changes, and never use search or detail as an
unconditional future write permission. No restore/provider/native authority or
Reader/model activation follows.

Implementation source: [maintenance and writer](../src/memory/maintenance.rs), [Key/Fact/Inspection](../src/memory/mod.rs), and [selected projection](../src/memory/aggregate.rs).
