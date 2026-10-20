# Normalized source retrieval API

This typed API receives material that its caller has already allocated. File/network adapters must bound decoding and acquisition before constructing it; the index's validation limits are not a streaming input-allocation guarantee.

`retrieval::{NormalizedDocument, Index, Cache, Provider}` is a bounded local library seam. It does not extract a PDF/EPUB/notebook, discover the current tablet document, change Reader prompts or perform research. Source-format labels record the extracting adapter's assertion; they do not establish supported extraction or native authority. Its central capability is `normalized-source-index` in ReMarkableBuddiesDocs; broader Reader retrieval remains REM-4 work.

Supply a non-nil document UUID, explicit PDF/EPUB/notebook format, lowercase SHA256 of original source bytes, an extractor revision and ordered source units. Each unit needs a positive page number or a nonblank section; repeated `(page, section)` locations refuse. The source digest is an adapter assertion, not proof of original-byte acquisition or extraction fidelity. The index separately hashes the complete normalized units, including location metadata, to prevent reuse after normalized material changes.

Input limits are 4 MiB total source text, 512 KiB per unit, 4,096 units, 512 bytes per section and 128 bytes per extractor revision. UTF8 source bytes remain unchanged; chunks are at most 2,048 bytes, prefer a whitespace boundary in their latter half and retain half-open original-unit byte ranges. Empty text units are allowed and contribute no passages. Invalid identity, digest, locations, unsupported controls or bounds refuse before index installation replaces an existing entry.

Keep the returned `IndexKey` and pass that exact key when querying. A changed document, format, original digest, extractor or normalized digest refuses rather than querying a stale index. Each query scans the full index anew; a follow-up can reach material outside the previous result set. Initial scoring counts distinct shared case-folded alphanumeric terms, with source order breaking ties. This is lexical ranking, not semantic retrieval, evidence verification or model effectiveness.

Query text is nonblank, at most 2,048 UTF8 bytes/no NUL, with 1–64 distinct searchable terms. Request 1–16 passages and 512–16,384 bytes of **complete serialized JSON**, including provenance and escaping. All direct candidates precede optional immediate same-unit neighbors. Chunks selected by multiple candidates are emitted once. Unrepresentable passages are omitted with `budget_limited: true`; a result that cannot fit even one direct match refuses. No matches returns an empty passage list with the source key, without a false incomplete flag. Passage text is never partially truncated to fit. Scores are deliberately not confidence estimates.

`Cache` holds at most 16 document indexes in process. Installing identical material retains the previous index; different material atomically replaces that document's index after validation/building succeeds. A failed install preserves the previous entry. This is derived memory cache only, not a disk-cache, Store, sync or restore implementation. The provider trait allows the receiving code to substitute a retriever independently of its answering model.

The separate [owned local source cache](normalized-source-cache.md) can persist validated normalized inputs and rebuild this Index after restart. Automatic Reader ingestion, PDF/notebook extraction, turn-linked receiving context and durable citation evidence, Reader cache integration, semantic/reranking choices where useful, default external research with no-internet policy, and real-provider/native qualification remain unfinished central tasks. In particular, a historical source UUID and a valid index do not authorize tablet navigation or answer-page effects.

## Offline EPUB text adapter

`retrieval::epub::normalize(document_uuid, reader)` produces an `EpubText` containing
the existing normalized document and ordered `textless_sections`. It reads a
bounded private copy, hashes the original accepted bytes, follows the first OCF
package and its declared spine (including auxiliary entries), and keeps canonical
archive section paths with no invented page numbers. Every spine unit remains in
order, including textless/image-only sections. An entirely textless book refuses.

This supports local XHTML body XML text: Unicode, CDATA, predefined/numeric
entities, inline text and block boundaries. Head/script/style text is excluded.
It is not CSS/rendered visibility, OCR, PDF extraction, layout fidelity, a complete
EPUB conformance validator or active tablet source identification. No archive file
is extracted and no URL or external filesystem resource is fetched.

Limits are16MiB original bytes plus one refusal sentinel;4096archive entries;
2MiB metadata reads;64MiB cumulative archive reads and32768I/O calls;64KiB each
container/package XML;1MiB per XHTML and8MiB total decoded resources; XML1.0 UTF8,
depth64/events100000 per resource;512spine units;512KiB per normalized unit and
4MiB total text. ZIP and XML parsing use the pinned maintained libraries. These
byte/I/O admissions are not a hard process-memory/CPU deadline or a guarantee that
a caller-supplied blocking reader will return. Unsupported/ambiguous archives,
DTD/custom entities, encryption, unsafe paths or exhausted bounds return an error
without partial normalized output.

For explicit development ingestion of a regular file:

```sh
cargo run --example normalize_epub_source -- DOCUMENT_UUID book.epub > normalized.json
```

The example prints bounded JSON only after complete normalization. It does not
install an index, persist a cache, select a conversation source or call a model.
Use `Index::build(result.document)` and keep its returned key for local queries.
Reader integration and broader retrieval/research acceptance remain unfinished.
