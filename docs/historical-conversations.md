# Historical conversation library

`conversation::Ledger` stores schema-one historical Reader/Writer facts over an
`Arc<storage::Store>`. It is a library API; the installed Reader and provider do
not automatically use these records. Broader conversation integration remains
unfinished. Requirements and acceptance live in the central Docs change
[`historical-conversation-ledger-main`](https://github.com/s116821/ReMarkableBuddiesDocs/tree/docs/rem-37-historical-ledger-main/openspec/changes/historical-conversation-ledger-main).

Create a conversation, read its exact expected heads and append with a stable
operation ID. Identical retries return the historical acknowledgment; they do not
restore a deleted root or authorize a device action. A different request under the
same operation ID or a stale head set refuses. Store ownership and atomic commit
rules still apply. This ordinary schema-one API does not provide selected causal
views, mode admission, pending handoffs or settlement permission.

`prepare_request` validates full source-image decoding, dimensions and geometry,
stages all original/parent bytes and atomically records the request before
returning ordered stored images. It does not dispatch a provider request.
`prepare_sdk_fixture` stores explicitly synthetic SDK capture facts and image
bytes. Persisted document/page/session labels and completion/binding receipts are
historical facts, not proof of a current native observation or physical output.

`inspect` refuses conflicts, unsupported relevant records, incomplete committed
storage and malformed allocated chronology. Context assembly keeps exact eligible
turn text and correction links. Byte, turn and caller-counted token bounds require
an explicit range when exceeded; an unknown provider token bound refuses. No
summarizer or provider tokenizer is supplied. Explicitly omitted imported media is
reported as unavailable rather than reconstructed or mistaken for complete media.
Retained-media inspection includes historical revisions after logical deletion;
it does not garbage-collect objects.

Export associations use a stable marker and small metadata record. Reconciliation
requires an exact unique backend/scope/revision match; uncertainty and conflicting
matches remain explicit. This library does not discover or create native notes.

The SDK dependency is pinned with default features disabled. Its mock feature is
enabled only for regression tests; ordinary build features must be checked
separately because Cargo test builds unify development features. The historical
mapper's `SDK_SOURCE` is provenance for its serialized schema, not a claim that
the current dependency revision is that historical source.

Public offline checks from the Rust checkout:

```text
cargo test --locked --test conversation --test conversation_sdk
cargo tree --locked -e normal,build,features -i remarkable-open-sdk
cargo check --locked --lib --bin reader-buddy
```

The tests exercise real local Store publication/reopen/recovery with declared
images and synthetic SDK captures. They do not establish real handwriting
interpretation, paid-provider behavior, live device timing, native authority or
physical-panel correspondence. Run the repository's full tests and strict Clippy
for integration validation; use the central testing checklists for later runtime
or native integrations.
