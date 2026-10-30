# Historical notebook page-image normalization

`retrieval::notebook::normalize` explicitly transcribes caller-supplied historical
PNG bytes using the existing replaceable `LLMEngine`. It does not acquire a native
notebook, inspect the device, change originals or activate installed Reader paths.
The model's transcript, blank-page judgment and caller model identity remain
unverified. An unreadable page refuses the complete set rather than quietly
indexing a partial transcript. No expected text or query is included in the OCR
prompt. Image instructions are treated as data; the prompt is not a proof of
resistance to every malicious image.

The caller supplies a document UUID and1..4 ordered positive unique page numbers
with PNG bytes. Reading order is list order, independent of numeric page order.
The versioned digest covers the domain `notebook-page-image-set-v1` plus zero byte,
big-endian u32 count, then each page's big-endian u32 page/u64 byte length and exact
PNG bytes. This identifies this historical image set/mapping, not native notebook
file bytes or whole-notebook completeness. The existing `IndexKey` separately
retains document UUID, formatNotebook and normalized-text digest.

`extractor_revision` starts `notebook-ocr-unverified-v1:` followed by SHA256 of
the deterministic profile serialization (protocol, prompt revision, caller model
profile). This visible origin survives existing Index/cache/query without schema
changes. Profile identity is caller-declared, not authenticated by the engine.
Changed bytes/order/page mapping, profile or normalized material invalidate the
corresponding complete key. Caching does not certify OCR or establish freshness.

Default ceilings are8MiB/page,16MiB total original PNG,4096 pixels/side,
8million pixels/page and16million total. Complete PNG chunk framing/CRC and final
IEND are checked; at most65,536 chunks/page and no APNG. Actual PNG decode uses
explicit64MiB allocation limits plus admitted dimensions/output size. The image
library describes allocation limits as best effort; this is not a hard process RSS
limit. Each decoded buffer is dropped before the next image. Options can only
lower ceilings and inconsistent/zero positive bounds refuse.

Complete ordered text/image-content JSON is bounded to24MiB; its digest/encoded
byte count are returned. This is not a model image-token/window/billing estimate
or full HTTP envelope. One engine invocation handles the set, without adapter
retry/repair/fallback. Response128KiB, page text32KiB, all text128KiB and complete
serialized report256KiB ceilings apply. Strict JSON requires only ordered `pages`
entries with exactly `page`, `status`, `text`; statuses are `transcribed`, `blank`
or `unreadable`. Duplicate/extra fields, invalid locations/order, controls,
truncation, empty transcribed text or text on a blank page refuse.

Temporary engine text/images are cleared before admission and on every exit,
including unwinding, assuming `clear_content` does not panic. Progress checks occur
around decode/dispatch/response/return and through cooperative engine progress.
Callback failure is latched even if an engine swallows it. Arbitrary synchronous
engines are not forcibly preemptible; default progress only checks before execute.
No physical/remote erasure, source fidelity, model nonmemorization or native
authority follows. The adapter writes no files or cache slots.

## Maintained actual consumer

Build `cargo build --no-default-features --example notebook_ocr_check`. The
example requires explicit `--live`, new absolute `--output`, repeated `--image`
and matching `--page`, one or more `--query`, and an explicit/default model. It
constructs the actual Index, independently retains its key, installs into a
temporary owned Store/DiskCache, drops/reopens the store/cache and queries with
that exact key, checking page and UTF8 ranges. The temporary Store is removed;
caller inputs are preserved. The explicit private report retains bounded raw
model return, normalization, key/results and matching actual content digests.
Console errors omit model/source/provider bodies and credentials.

Example with the fixed [genuine handwriting fixture](../tests/fixtures/notebook-handwriting/README.md):

```text
cargo run --no-default-features --example notebook_ocr_check -- --live --output ABSOLUTE_NEW_PRIVATE_REPORT.json --image tests/fixtures/notebook-handwriting/page.png --page 9 --query "Mattie Walden" --query "Junior class"
```

Credentials come only from the process or a `.env` in the current directory;
never print/copy them into reports or commands. The engine default model is
explicitly visible in `--help`; this example reads no saved model declarations.
Configured nondefault `OPENAI_BASE_URL` requires explicit `--base-url`, with HTTPS
host-only or numeric-loopback HTTP; the existing engine appends its endpoint.
Timeout is1..90seconds. `--max-input-bytes`, `--max-page-text-bytes` and
`--cancel-after-ms` are bounded development controls, not product settings.

Run `cargo test --no-default-features --test notebook_normalization` for synthetic
policy/cache controls and `python tests/fixtures/notebook_ocr_example_check.py
ABSOLUTE_EXAMPLE_BINARY` for13 actual engine/loopback-HTTP subprocess controls.
They use dummy credentials and synthetic pixels/text, deny external forwarding
and join owned servers. They do not qualify handwriting accuracy. A separately
inspected bounded hosted fixture observation is required for recognition evidence;
source/build, local transport, hosted recognition and native/runtime qualification
are distinct. The central change is `normalize-notebook-page-images` in Docs.
