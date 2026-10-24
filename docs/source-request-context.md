# Historical Reader request context

`Ledger::source_request_context` assembles a read-only, immutable request frame
from existing ordinary conversation records and a local `retrieval::Provider`.
It does not dispatch a model, write Store records, identify a live tablet page,
or admit selected Reader effects. The central capability is
`source-request-context` in ReMarkableBuddiesDocs.

For explicit durable publication, use the separate
[request-evidence API](request-evidence.md). This receiving API remains read-only.

The request must already be a nonblank interpreted Reader user turn. The frame
keeps that request separately from verbatim eligible history through its sequence;
future turns never enter it. An explicit `TurnRange` must include the request and
end immediately after it. Without a range, all eligible history through the request
is included. Existing context eligibility and correction references remain intact.
Limits refuse rather than silently shortening the request or history.

The caller must choose `RequestSource::NoDocument` or `RequestSource::Original`.
No-document is an explicit declaration, not proof that a page is blank, and makes
zero retrieval calls. Original supplies an existing user turn, its linked source
record UUID, and an independently obtained complete `IndexKey`. The source record
must name that same conversation and original turn; its recorded document must
match the key. A follow-up explicitly reuses this reference and supplies the fresh
expected key. Each bound request queries the whole local index anew. No association
is automatically discovered, persisted or inherited. The current Buddy page and a
cache's own key cannot establish original-source identity or freshness.

ImageUse, SDK historical capture and development capture records retain their exact
historical metadata and qualification. Development document labels must be canonical
nonnil UUIDs. Legacy capture has no identity to bind. None of these records proves
current native identity, extraction fidelity or source freshness. The original
association is independent of current capture-batch membership; `Turn.sources` and
`prepared_images` rules are unchanged.

`ReceivingBudget.context` includes the request: 1–64 turns, 1–65,536 text bytes and
an explicitly known positive caller token limit. `max_encoded_bytes` is 512–524,288;
`max_passages` is 1–16; `max_passage_bytes` is 512–16,384. The complete serialized
frame includes request, history, full original record, key, passages, escaping and
missing-media descriptors. Serialization refuses before appending past its byte
cap. The initial frame contains `retrieval:null`; its four bytes are reclaimed when
reserving the provider result. Remaining room below 512 bytes refuses before query.
The final frame is counted again with the supplied counter and must fit both limits.
This is not a model prompt/window or global CPU/RAM guarantee. A byte-count fixture
counter is synthetic, not a tokenizer or a token-to-byte estimate.

Provider errors, stale/wrong keys, malformed passage ranges/locations, unsupported
controls, duplicates, oversized results and exhausted bounds refuse the entire
frame. There is no retry, partial fallback or conversion to no-document. Honest
zero matches retains a source-bound empty retrieval result. Source/key/content
fidelity beyond these structural checks remains the Provider's contract.

`FrozenRequestContext` exposes immutable getters and stable `encoded_json()`.
It includes explicit missing-media descriptors, including original media outside
the selected history range; it embeds no image bytes and performs no OCR or answer
reconstruction. Records are captured before token/provider callbacks. Later record
replacement cannot swap new metadata into that frame. Media availability uses the
existing ordinary Store object checks; this is not a selected logical-coverage
admission. Caller callbacks may have their own side effects; the receiving API
itself performs no writes.

For a reproducible local development fixture, run:

```sh
cargo test --locked --test source_request_context
```

These authored fixtures use actual temporary Store and Index queries, independent
keys, source changes, follow-ups reaching different pages, no-document zero-query
checks, an imported omission outside the history range, exact byte/token boundaries,
callback record replacement and adversarial Provider outputs. SDK/development facts
are synthetic historical fixtures. No tablet, external provider or live model is
contacted. Durable passage/citation evidence, automatic inheritance, router/Reader
integration, model dispatch and research policy remain unfinished.
