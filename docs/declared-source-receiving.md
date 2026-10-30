# Receiving a declared source

The ordinary ledger offers two optional entry points using the existing
`conversation::resolution::SourceDirective`:

- `source_request_context_with_directive` returns the existing immutable
  `FrozenRequestContext` without writing records.
- `prepare_request_evidence_with_directive` returns the existing sealed
  `PreparedRequestEvidence`. Publish separately through unchanged
  `persist_request_evidence`.

Their other arguments match the [read-only receiver](source-request-context.md)
and [durable preparation](request-evidence.md). All existing entry points remain
available, with their existing errors, admission order and serialized data.

These optional methods resolve the source from the same validated ordinary
records used for frame assembly. They do not call the standalone resolver and
then inspect another snapshot. Explicit choices use receiving's original
validation; inherited choices require a specifically named earlier request and
evidence operation in the same conversation. An omitted operation is unresolved
even with one candidate. Other operations, later timestamps and the current Buddy
page never select a source.

Inherited original sources retain exact original turn/source identity and require
an independently supplied complete `IndexKey`. The current captured source content
must match the named historical choice; identical-content revision successors
are allowed. Independently reindexing the same document may produce a different
key. The Provider still validates the actual index; resolution cannot prove how a
key was acquired or infer native freshness from it. Inherited no-document requires
no key, and explicit no-document deliberately clears the association.

Bound receiving queries the local Provider once using the exact current request;
no-document makes zero queries. History, explicit ranges, original provenance
outside a narrowed range, missing media and full encoded/token budgets keep their
existing meanings. No history is rewritten or truncated to force success.

Provider and token callbacks run outside Store locks. If they change current
history, sources or the named fact, a read-only frame still contains its captured
facts. That is historical consistency, not a promise that current state stayed
unchanged. A durable preparation captures the relevant anchor/fact/receipt and
other heads plus the Store generation. Fresh publication refuses relevant changes,
including identical-content revision changes, without re-resolving, requerying or
partially publishing. Unrelated conversations remain independent.

Source-resolution refusals use sanitized `SourceResolutionRefusal` errors. The new
preparation maps capture/history failures to `HistoryUnavailable`, without raw
internal path or metadata chains. Receiving and context-budget errors retain
their existing typed meanings. Legacy preparation's error behavior is unchanged.

Frames remain schema 1 and request evidence remains version 1. Saved evidence
records the resolved source/frame, not the directive or operation that caused
inheritance. It is not directive lineage, active user intent or model consumption.
Equivalent explicit/inherited frames keep existing retry equality based on
frame/count/limits/timestamp. Supported historical recovery returns an original
committed acknowledgment after restart/deletion without another write or query;
fresh preparation still requires valid current dependencies. Reading historical
evidence never creates a writable preparation or resurrects a deleted root.

Existing bounds apply: 64 turns, 64 KiB context text, 512 KiB complete frame,
16 passages / 16 KiB complete retrieval result, positive caller-reported token
limits, and bounded original metadata/reference work. A durable envelope must
still fit 1 MiB including escaped frame JSON. Combined maximum inputs can refuse.
These are data/work bounds, not a global execution deadline or model-window proof.

Run the focused offline fixtures with:

```text
cargo test --locked --test request_evidence declared_receiving
```

They use real temporary Store/Index data, callback mutations, unchanged legacy
frames, refusal cases, metadata-only imports and exact historical references.
They qualify this ordinary data path, without automatic runtime source selection,
Reader/Writer/router integration, live model, native identity or tablet claims.
