# Durable request evidence

`conversation::evidence` records the context explicitly selected for an existing
interpreted Reader request. It preserves the exact receiving JSON, original
source/key/passages, caller-reported token count and limits, and original committed
record references. This is historical selection evidence; it does not establish
model consumption, current document freshness or native authority.

Call `Ledger::prepare_request_evidence` with the same arguments as
`source_request_context`. The preparation has private fields and exposes only its
immutable `context()`. Bound selection queries the supplied local Provider once;
explicit `RequestSource::NoDocument` makes zero queries. Invalid associations,
Provider failures and unknown/exhausted counters refuse preparation. Source-bound
zero matches and unavailable media remain distinct facts.

Call `persist_request_evidence(operation, recorded_ms, &prepared)` separately to
publish the fact, root successor and ordinary receipt atomically. Use a stable,
non-nil operation UUID for retries. Fresh publication checks the originating Store
instance, generation and complete relevant heads, including same-content revision
changes. Counter and Provider callbacks run outside Store locks. Unrelated
conversations can change independently. A stale preparation refuses; obtain a new
preparation rather than silently requerying or shortening context.

An identical committed retry returns the original acknowledgment, including after
a lost acknowledgment, concurrent winner, restart or logical deletion. Reusing an
operation for different frame/count/limits/timestamp refuses. Historical retries
do not recreate a deleted conversation. A deliberate new selection requires a new
operation. The root's sequence allocation and binding remain unchanged.

`request_evidence(conversation, request_turn, operation)` reads a known committed
fact without a Provider or tokenizer. Its result contains the historical record,
original acknowledgment and separately reported present media unavailability.
Recorded missing-media facts remain unchanged. Readback validates strict canonical
frame encoding, exact original object references, complete through-request turn
allocation, source relationships and the original three-record transaction.
Inspection and settled-history cleanup apply the same domain checks. Unsupported,
missing, contradictory or corrupt required metadata refuses interpretation.

Ordinary complete export/restore preserves these facts. Metadata-only imports keep
logical records and report omitted images. Imported envelope actors need not equal
the receiving Store's actor. References identify original committed bytes, even
when imported JSON used different whitespace; parsed reserialization is never
substituted for original identity. Logical deletion retains historical evidence.

Receiving limits remain 64 turns, 64 KiB text, 512 KiB complete frame, 16 passages
and 16 KiB complete result, plus the declared positive token limit. Captured heads
and saved references admit at most 4096 records / 8 MiB, with each original record
at most 1 MiB. The entire durable envelope must fit 1 MiB, including escaped frame
JSON and provenance; a valid receiving frame can therefore refuse persistence.
Reference validation admits 8 MiB of work / 8192 visits per bounded read, caches
duplicate references, and charges both the bounded original file read and content-hash pass. Combined
maximum inputs may refuse. These are data/work bounds, not global CPU/deadline or
model-window guarantees. Nothing is truncated to make a write succeed.

This API does not add a source registry, capture batch, automatic inheritance,
Writer normalized-text implementation, selected projection or runtime/model route.
The existing [read-only receiving API](source-request-context.md) remains available
without publication.
