# Export explicitly prepared notes to Mem

The `note_export` library and `reader-buddy --export-note` command accept prepared
Markdown and source provenance. They do not extract handwriting, call a model,
start Reader, initialize tablet input/rendering or expose a Buddy network service.
The command makes a real external write only when explicitly requested with
valid content, credentials and an available owned Store.

Set `MEM_API_KEY` through a protected process/service environment. There is no
token command-line argument; tokens and note bodies are absent from reports.
`REMARKABLE_BUDDIES_CONFIG` selects the existing nonsecret configuration and Store
paths. The command requires the normal exclusive Store OS lease; it cannot bypass
a running process's ownership. In-process callers use the existing `Ledger`
through `note_export::export` or `reconcile` instead of another Store or service.

## Commands

Send one JSON object on stdin, at most 256 KiB. This example uses invented IDs;
the conversation must already exist in the selected Store:

```json
{
  "command": "export",
  "note": {
    "conversation": "10000000-0000-4000-8000-000000000001",
    "correlation": "20000000-0000-4000-8000-000000000002",
    "effect_operation": "30000000-0000-4000-8000-000000000003",
    "source_scope": "notebook-id/page-id",
    "source_revision": "prepared-revision-1",
    "title": "Study notes",
    "markdown": "Prepared content with useful page/coordinate provenance."
  }
}
```

Reuse the correlation for the same exported note. A deliberate new export uses a
new effect-operation UUID and may supply a changed source revision/content.
An identical operation retry cannot send another write. To reconcile an uncertain
effect without resending its body:

```json
{
  "command": "reconcile",
  "conversation": "10000000-0000-4000-8000-000000000001",
  "correlation": "20000000-0000-4000-8000-000000000002"
}
```

The command emits one JSON report. Inspect `status` and optional `failure`; a
successfully emitted refusal report is not a successful export. Reports contain
identities and sanitized result codes, never the input content or token.
`verified-current` confirms that the destination was observed to contain the
exact desired content; it does not prove a particular HTTP call executed.
`historical-completed` reports an earlier completed operation without claiming
the remote note is still unchanged. `busy` makes no second call. `conflict`,
`incomplete`, `refused` and `reconcile-required` do not authorize another write.
`canceled-no-effect` means this attempt positively did not dispatch; a new
deliberate effect UUID may proceed under current state. Ctrl+C after dispatch
cannot undo an external effect and may instead require reconciliation.

## Destination and persistence rules

Mem uses its official HTTPS `/v2/notes` API with the existing exact
`[RMB:<correlation UUID>]` title suffix. Prepared Markdown and its useful
human-readable provenance are preserved without an added machine-ID footer.
The first Markdown line is the Mem title. A separate backend
may implement the replaceable `NoteDestination` metadata capability using
`remarkable_buddies_export_id`; arbitrary Mem metadata support is not assumed.
There is no fuzzy title/prefix match or second association registry.

The exporter prepares one attempt in the existing Ledger before any write.
Known note IDs use direct verified reads. Unknown correlation uses at most 20
chronological listing pages of 100 rows; search's relevance window is not an
absence proof. Changed totals, duplicate/repeated identities, ambiguous markers,
trash or exhausted bounds refuse. A discovered remote note without a verified
local baseline is not silently adopted or overwritten.

For updates, both the current body digest and version must match the previous
confirmed export before a full-body, exact-version PATCH. External edits require
explicit refinement; the exporter does not merge arbitrary user changes. It
avoids a PATCH when verified readback already contains the exact desired body
with a consistent version. That result describes observed content, not a newly
performed mutation. Conversation/effect identities, source scope/revision and
prior/desired body hashes stay in the existing Ledger. It
never retries a create/update automatically. A client-selected create UUID is a
reconciliation aid, not an assumed duplicate-POST guarantee. Uncertain attempts
are reconciled by exact ID/marker/body and cannot start a successor cycle.

Attempt-bearing export envelopes use domain schema 2 only in the export
association namespace. Older schema-one records retain their original shape and
remain readable; they do not acquire a verified remote-body baseline. Old clients
must refuse domain 2. Store/envelope format remains 1, with the supported
namespace/version combination in `storage/contract-v1.json`. Retained revisions
and immutable stage receipts supply bounded old-operation lookup after later
exports without restoring old current state. A shared in-memory owner serializes
all export/reconciliation invocations within the same Store; the Store index lock
does not span HTTP.

The complete generated content is at most 200,000 UTF-8 bytes, title 256 bytes,
scope/revision 1024 bytes each, response 1 MiB and cursor 4096 bytes. The original
45-second operation deadline bounds request admission; each request uses at most
10 seconds and the remaining budget. Synchronous Store I/O is not preemptible.
Production disables redirects and automatic retries. The explicit loopback-only
fixture constructor is for local real-HTTP checks, never a production origin
override. No live Mem account or tablet qualification follows from these fixtures.

Source basis: the
[central contract](https://github.com/s116821/ReMarkableBuddiesDocs/blob/main/openspec/specs/prepared-note-export/spec.md),
current implementation and focused HTTP/Store checks;
Mem's [create](https://docs.mem.ai/api-reference/notes/create-note),
[read](https://docs.mem.ai/api-reference/notes/read-note),
[update](https://docs.mem.ai/api-reference/notes/update-note) and
[list](https://docs.mem.ai/api-reference/notes/list-notes) documentation. These are
prepared-content export semantics, not whole Writer generation/refinement or
native delivery acceptance.
