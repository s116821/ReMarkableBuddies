# Historical source and optional memory context

`FrozenRequestContext::with_memory` consumes an existing frozen source frame and
returns `FrozenSourceMemoryContext`. It combines data only: no Ledger read, source
query, record publication, model prompt/call, device action or configuration lookup.
The central capability is `source-memory-context` in ReMarkableBuddiesDocs.

The separate [declared research stage](declared-research-context.md) can consume
this frozen object while preserving its exact bytes and historical classification.

Import `conversation::receiving::memory::{MemoryInput, MemoryOptions,
CombinedBudget}`. Supply `MemoryInput::Disabled`, or `MemoryInput::Query` with an
existing `memory::Provider`, explicit nonnil profile and options. Enabled query text
is the exact captured interpreted request; document is its original IndexKey's
document, or None for explicit NoDocument (global memory only). The caller supplies
time and canonical current-claim assertions. Source text is never interpreted as
trusted suppression claims. Query limits reuse the existing memory contract:
1–16 matches, 256–16,384 context bytes and at most 64 canonical claims.

Memory query text is limited to 2,048 UTF-8 bytes. A larger NoDocument frame can
still be used by the source-only receiver or disabled composition; enabled memory
returns typed `CompositionRefusal::IncompatibleRequest` without truncating it or
querying the provider. Disabled needs neither profile nor provider and makes no
query. Enabled makes one query, without retries, after option/byte preflight.

`source_context()` retains the original object, including byte-identical
`encoded_json()`, exact request/history/source/passages and missing-media evidence.
The combined transient JSON nests that same schema-1 frame under `request_context`
and adds `memory` with profile, exact query, status, diagnostics and labeled context.
Disabled has status disabled with no query/context. Valid CompleteEmpty and Partial
(including empty Partial) preserve their different diagnostics. Unavailable,
selected-pinned, malformed, foreign, expired, suppressed or oversized returned facts
refuse; unavailable does not become empty success. Assertions and UUID provenance
remain author-supplied, without independent source verification.

`CombinedBudget` permits 512–524,288 complete encoded bytes and a positive explicit
caller token limit. The capped serializer includes both layers, labels, status,
diagnostics, query identity and JSON escaping. A `memory:null` wrapper proves only
a byte lower bound before query. The supplied counter is invoked once on the final
encoding: tokenization need not be monotonic, and component counts are not added.
Zero/error counts or exhausted limits refuse, without dropping memory or rewriting
history to fit. `counted_tokens()` is the caller's count, not a model-window claim.
Adapters supply already-allocated typed data; output bounds do not bound their
acquisition, process RAM, runtime or provider-specific tokenization.

Source and memory are separate historical observations, not an atomic joint Store
snapshot. Callbacks may mutate underlying data; they cannot replace captured source
metadata. No currentness lease or publication/effect permission follows. The combined
object cannot become a writable request-evidence preparation. Existing source-only
and durable evidence APIs and schemas are unchanged. Future model/runtime consumption
retains its separate validation and live-model gates.

Local controls use actual temporary Store/Index/LocalMemory and an actual selected
memory provider, plus injected malformed results and reentrant callbacks:

```sh
cargo test --locked --test source_request_context memory::
```

These authored fixtures and synthetic byte/token counters contact no tablet,
model, network provider or service. They qualify this data boundary only.
