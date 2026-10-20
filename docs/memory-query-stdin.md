# Subject-memory query over stdin

The one-shot maintenance interface accepts:

```text
reader-buddy --memory-operation query --memory-input -
```

Set `REMARKABLE_BUDDIES_CONFIG` to the existing supported absolute configuration
path. Send one UTF8 JSON document to stdin and close stdin (EOF). No model key,
running Buddy service, external model/provider call, temporary query file or tablet input is
required. Ordinary absolute regular-file input continues to work. `/dev/stdin`
is not a substitute: existing file safety checks deliberately reject it.

## Request

This is the existing Query schema, not a transport wrapper. Example values below
are illustrative; construct `now_ms` from the caller's current Unix milliseconds:

```json
{
  "document": null,
  "text": "physics gravity units",
  "now_ms": 1792060000000,
  "current_claims": [],
  "max_matches": 8,
  "max_context_bytes": 8192
}
```

Do not send profile identity; the configured memory profile is authoritative.
`document:null` is the Manager's profile/global search. An explicit document UUID
must be nonnil and is not inferred from the currently visible page. Text is
nonblank, at most2048 UTF8 bytes and contains no NUL; multiline/Unicode text is
preserved. `current_claims` is at most64 existing canonical claims. Match count
is1–16; context budget is256–16384 bytes. Unknown fields and invalid domain input
are rejected by the same validator as file mode.

The entire stdin document is at most1048576 bytes. The reader consumes at most
1048577 bytes to detect oversize. Malformed/invalid UTF8 JSON, trailing nonspace
JSON, invalid fields, read failure and oversize return sanitized `invalid-input`
before Store admission. Ordinary missing configuration/profile can return
`unavailable` before reading stdin. No input or I/O error text is echoed.

This guide describes query. `inspect` also accepts the marker through the strict
[single-fact inspection wire](memory-fact-inspection-stdin.md). Put/setup
operations return `unsupported` before reading stdin, loading configuration or
opening Store. [Prepared forget](memory-prepared-forget.md) is a separate explicitly
confirmed protocol; query success supplies no mutation authority. Existing regular-file
interfaces are unchanged.

## Response and currentness

Report version remains1. Exactly one bounded UTF8 JSON report plus newline is
written to stdout; the main path uses the existing guarded `Report::write_to`.
A healthy empty example is:

```json
{
  "report_version": 1,
  "operation": "query",
  "status": "ok",
  "incomplete": false,
  "data": {
    "query_status": "complete-empty",
    "diagnostics": {
      "conflicted_keys": 0,
      "contradicted_claims": 0,
      "unsupported": 0,
      "expired": 0,
      "suppressed": 0,
      "budget_omitted": 0,
      "snapshot_unavailable": false
    },
    "context": {
      "kind": "associative-memory-data",
      "provenance_authority": "author-asserted-not-source-verified",
      "facts": []
    }
  }
}
```

Populated `context.facts` retains actual record/revision IDs and typed fact
keys, scope, provenance and confidence. Preserve `partial`, `busy`, `unavailable`,
`unsupported`, `invalid-input`, query status and `incomplete` honestly; empty
facts alone do not prove a successful empty search. Selected results retain the
existing current-view check at actual stdout handoff. A stale/forged report
refuses, and an I/O failure after a written prefix never appends another report.

Generic report bound remains8454144 bytes, including escaping/newline. A client
may impose a smaller cap such as128KiB for its fixed8-match/8192-context request;
it must reject overflow/partial/non-JSON/nonzero-exit output, not truncate it or
display a healthy empty result. Do not assume the generic bound was reduced.

## Completion, cancellation and older builds

EOF is required. The byte cap is not a wall-clock deadline; the caller owns its
existing child/channel deadline and cancellation. Truncated JSON on EOF refuses.
Closing a host channel after valid dispatch is not proof that no query or normal
Store recovery occurred. Domain-read-only search is not filesystem immutability.

Older regular-file-only builds reject `-` as an unsafe relative path before
Store admission. A known well-formed request receiving that unsupported-input
response is an unsupported/unavailable client state, not an empty search. Do
not retry through `/dev/stdin`, an arbitrary remote staging file or a new service.

These are host maintenance semantics. This interface does not establish native
page identity, sync/provider readiness, Writer behavior or automatic activation
of settings in a running application.

## Storage mode admission

Host maintenance requires legacy sync to be disabled. Local memory admits only
an absent mode choice or LocalOnly mode. Selected memory requires an already
chosen SharedGroup mode matching the explicit selection group and binding.
Foreign and IsolatedBackup modes refuse; this CLI does not choose modes or start
sync workers. Explicit aggregate/catalog setup initializes domain records only.
The command holds mode authority and any legacy-data lease through actual report
output and failure cleanup. It releases them when the report/execution ends.
The configuration's memory.enabled setting remains a preference for the separate
Reader integration; explicit host maintenance is allowed with that preference
off. This delivery does not activate memory in the normal Reader loop.
