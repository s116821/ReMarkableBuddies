# Local normalized source cache

`retrieval::disk::DiskCache` reuses validated normalized inputs across process
restarts. It borrows an already-open `Store`, uses that Store's owned cache root
and rebuilds the existing `Index` instead of persisting private chunks or terms.
This library does not acquire source text or connect retrieval to Reader/model
operations. A cached source digest is still an adapter assertion, not proof that
the document is currently visible or unchanged.

Open one handle with `DiskCache::open(&store)`. Its versioned namespace is
`store.paths.cache/normalized-sources-v1`. The namespace owner must match the
Store actor. Opening may initialize the owner/lock files and clean abandoned
shared-helper staging files after ownership and lock checks; it is not
filesystem-immutable inspection. A second handle refuses immediately. Operations
on the same handle are serialized. The handle borrows the Store lease throughout
its lifetime and explicitly releases its own namespace lock when dropped.

`install(normalized_document)` validates/builds and encodes the complete candidate
before atomic replacement, returning its complete `IndexKey`. Invalid input or
capacity refusal leaves the prior slot intact. Shared atomic publication writes,
syncs and renames a bounded staging file, then syncs the parent directory. A
post-rename directory-sync error can report failure after publication; it does
not prove rollback. No second persistence/conflict engine is introduced.

`load(&expected_key)` requires a key independently selected by the caller's source
workflow. A missing slot returns `None`. A stale, corrupt or unsupported entry
refuses. The complete stored wrapper and material are validated and the rebuilt
key must equal the expected key before returning an Index. Do not read a cache's
own key and use it to assert current source freshness. Query the returned Index
using that same key and the existing passage/count/serialized-byte budgets.

An already-open `DiskCache` also implements `retrieval::Provider`, so ordinary
declared receiving can take `Some(&cache as &dyn Provider)` directly. Each query
calls the existing `load` once and forwards the exact expected key, question,
passage count, complete result-byte budget and neighbors flag to the rebuilt
Index. A missing slot is an error here, not an empty result or no-document choice.
Valid material with no matches still returns the source-bound empty result.
No cache is opened implicitly and querying does not install, forget or memoize
material. The load operation lock is released before searching the owned Index.
A later replacement cannot rewrite that immutable observation; each later query
reloads independently. This is not a cache-currentness lease, original-source
freshness proof or new durable receiving publication admission.

There are at most 16 document slots, with at most 32MiB complete encoded JSON per
admitted slot. One additional bounded staging file may exist during replacement.
Reads are capped before JSON decoding; ordinary Index text/location limits still
apply. Directory enumeration and abandoned staging count are bounded. These are
logical byte/count admissions, not an OS disk quota, RAM/CPU deadline or bounds on
other cache domains. No automatic eviction, extraction or network request occurs.

`forget(document_uuid)` removes only that explicitly selected canonical slot in
the owned namespace and syncs the directory. Unknown entries and symlink/reparse
paths refuse rather than being adopted, traversed or broadly deleted. Original
sources, other cached documents/domains and durable Store records/media remain
outside this operation. Cached data is disposable and is not added to sync or
durable evidence; a miss requires caller-controlled reacquisition/rebuilding.

Original local fixtures cover reopen/real queries, complete stale keys, tampering,
oversized/unknown-schema input, rejected replacement, capacity/forget, lock and
Store lifetime, staging recovery and unknown-file preservation. Unix fixtures
also exercise symlink refusal. These tests do not establish installed Reader,
real-provider, cloud-sync or tablet behavior.

Provider fixtures additionally use the real cache through declared receiving
after reopening the Store, follow-up queries reaching other pages, replacement
and stale keys, missing/corrupt versus unmatched material, no-document bypass,
complete frame limits and unchanged slot bytes. Direct Index comparisons retain
ranking, exact provenance, neighbors, result budgets and refusal behavior. These
qualify the local integration seam; automatic runtime ingestion and model/native
consumption remain unfinished.
