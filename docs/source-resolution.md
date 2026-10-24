# Declared source inheritance

`conversation::resolution` is an ordinary read-only adapter over existing ledger
records. It helps a caller carry forward a specifically declared historical source
choice. It does not discover active user intent or wire automatic runtime
inheritance into Reader, Writer or the router.

Call `Ledger::resolve_request_source(conversation, request_turn, directive)` for an
existing interpreted Reader user request:

- `SourceDirective::Explicit(RequestSource::NoDocument)` deliberately clears the
  source. Explicit `Original` selects an existing linked turn/source reference and
  independently supplied complete `IndexKey` using ordinary association checks.
- `SourceDirective::Inherit { request_turn: prior_request, operation: Some(op),
  expected }` carries forward the selection in that exact earlier evidence
  operation. The prior request must be strictly earlier in the same conversation.
  Several operations may disagree; choosing one is the caller's declaration.
- `operation: None` returns `AnchorRequired` even when only one fact exists. It
  never means “pick the most recent.” Timestamps, IDs, page bindings and currently
  visible Buddy pages do not choose a source.

Inherited original-document selections require `expected: Some(fresh_key)` from
independent acquisition; inherited no-document requires `expected: None` and
returns explicit no-document. Missing/foreign/future/deleted/corrupt anchors and
invalid source relationships refuse with sanitized typed
`SourceResolutionRefusal` errors. Internal error chains are not exposed.

The named fact must remain present and equal in the current ordinary inspection
snapshot after historical readback. The current original source content must equal
the historical source content; an identical-content revision successor is allowed,
but changed source content refuses. The original evidence retains its immutable
historical record identities. Resolution does not promise identical current
revision identity or freeze state for another call.

The supplied key must have valid complete fields and the correct document ID.
It may differ from the saved key when that same document was independently
reindexed. Resolution cannot prove how the key was acquired or whether it is
fresh. A structurally valid stale key can resolve, and must still be refused by
the subsequent receiver/Provider if it mismatches the current index.

Pass the resulting `RequestSource` to the existing
[read-only receiver](source-request-context.md) or
[durable evidence preparation](request-evidence.md). Those APIs validate their own
snapshot, source/key, complete context and budgets; resolution does not bypass
them. Each follow-up queries the local source anew when the caller invokes
receiving. To carry that new selection forward again, explicitly record its
evidence and declare the chosen operation for a later request.

For one receiving snapshot, the optional
[declared receiving/preparation methods](declared-source-receiving.md) consume the
directive inside their own validated records. The standalone resolver retains its
separate-call boundary and does not freeze subsequent receiving.

Resolution makes no Provider/model/device calls, invokes no tokenizer and writes
no records. It reuses bounded historical evidence validation and ordinary
inspection (4096 records and existing metadata/reference work bounds), with a
512 KiB source-record encoding bound and 64 KiB request-text bound. Combined
inputs can refuse rather than truncate. Ordinary restart and metadata-only
imports retain historical selections even when media bytes are unavailable.

Tests use real temporary Store and Index fixtures. They qualify this ordinary
data API, not active-intent policy, live model consumption, native source identity,
tablet behavior, selected effects or integrated automatic inheritance.
