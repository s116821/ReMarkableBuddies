# Prepared cleanup of one derived source-cache slot

This additive one-shot producer supports Linux only. Windows and other targets
return unsupported before stdin, Config or filesystem admission; legacy cache
APIs are unchanged. Future browser and Electron consumers can invoke the same
Linux tablet producer through host adapters. This delivery does not add those
consumers or qualify an installed runtime.

Use an existing explicit Config through REMARKABLE_BUDDIES_CONFIG. No model key,
provider, service, original document or native operation is involved. The private
bounded requests below go through stdin, never arguments, logs or input files.
The input selector must be exactly `-`.

```text
reader-buddy --source-cache-operation inspect --source-cache-input -
reader-buddy --source-cache-operation prepare --source-cache-input -
reader-buddy --source-cache-operation apply --source-cache-input -
reader-buddy --source-cache-operation status --source-cache-input -
```

Inspect takes `{"request_version":1,"document":"CANONICAL_UUID"}`. Prepare adds a
fresh nonnil `operation` UUID. After reviewing the selected derived entry and
consequences, explicitly confirm apply with
`{"request_version":1,"intent":ORIGINAL_PREPARED_INTENT}`. Status takes exactly
the same original request for read-only reconciliation. Do not reconstruct an
intent from a later inspection or automatically retry after unknown/lost output.
All UUIDs use lowercase hyphenated spelling; all input objects are closed, strict,
complete JSON plus EOF, at most16384 bytes. Reports have the same bound.

The existing ordinary Store and normalized-sources-v1 namespace must have safe
ownership headers and stable existing nonblocking Store/cache locks. This route
uses a private header-and-lease admission, never Store initialization/recovery or
durable-history scans. Missing/unsupported/busy/changed state refuses without
repair. Unknown entries, staging and reparse paths refuse without cleanup.

Prepare binds original Config, paths, actor, ordinary generation, ownership and
namespace/entry identity plus exact bytes. Apply removes only the unchanged
selected UUID.json, syncs its directory and verifies absence while holding both
leases through output. It cannot erase source PDF/EPUB/notebook files, durable
history/media, memory or other cache domains. Reports contain only fixed fields,
UUIDs, hashes and lengths, never normalized source contents or freshness claims.

`applied` means unlink, directory sync and absent readback completed. `unknown`
with `possible_effect:true` means deletion may already have occurred. A failed
report write is also caller-unknown; no rollback/retry follows. Status distinguishes
`unchanged`, `absent` and `conflict`. Absence does not identify the historical
deleter. A changed/recreated entry is never deleted by stale confirmation.
Every report has runtime_unverified:true; exit0 means a complete report, including
refusal, while exit1 means report/transport failure.

The central proposed contract is tracked in [Docs84](https://github.com/s116821/ReMarkableBuddiesDocs/pull/84),
change prepare-source-cache-slot-cleanup. Linux synthetic fixtures and Windows
unsupported-boundary controls are separate from real-device/provider qualification.
