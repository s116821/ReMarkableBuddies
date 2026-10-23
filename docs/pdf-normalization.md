# Offline PDF text normalization

`retrieval::pdf::normalize(document_uuid, reader)` accepts caller-supplied bytes
and returns `PdfText { document, textless_pages }`. The document uses the existing
[normalized index](source-document-retrieval.md) and [owned cache](normalized-source-cache.md)
contracts. It hashes exact original bytes and records extractor revision
`pdf-simple-latin-r1-lopdf-0.46.0`. Every page stays in declared page-tree order,
including blank/image-only units; `page` is a one-based ordinal, not a printed
label or the tablet's current page. Entirely textless documents refuse.

## Supported profile

This first profile uses pinned MIT-licensed lopdf 0.46.0, without its default
parallel/clock features. It admits standard Latin Type1 Courier, Helvetica and
Times families with explicit StandardEncoding, WinAnsiEncoding or MacRomanEncoding.
Supported accented characters and ligatures become Unicode; unmapped bytes refuse.
There is no guessed encoding, replacement character, OCR or embedded-font fallback.
ToUnicode, CID/Type0, Type3, Differences, FontDescriptor and custom font programs
refuse before font-map decoding. Many ordinary PDFs use these unsupported fonts.

Contents must use unfiltered bytes or a single named FlateDecode filter without
DecodeParms. External stream resources are refused and never fetched. Images
may appear as Image XObjects but are not decoded. Form/PostScript XObjects,
annotations, AcroForm, ExtGState, optional content and Properties are unsupported.
This is text-layer acquisition, not rendering, layout fidelity, visible-text
proof, annotation/form extraction, source discovery or automatic Reader ingestion.

The closed content profile admits BT/ET, Tf, Tj/TJ, T*, apostrophe/doublequote and
numeric Tc/Tw/Tz/TL/Ts/Tr state. Only Tr=0 and positive font size/scaling are
accepted. Tf is required in every BT before shown text. Tm/Td/TD may position a
text object before its first shown text; subsequent arbitrary repositioning
refuses. T* and quote operations create newlines; ET ends a text boundary. TJ
negative spacing of at least120 text units inserts a space; smaller adjustments
preserve word joins. This lexical approximation is part of the extractor revision,
not a geometry or reading-layout guarantee. Whitespace is normalized and trailing
page whitespace trimmed.

Nontext operations are q/Q, cm, Image Do, numeric w/J/j/M/G/g/m/l/RG/rg/K/k/v/y/re/c,
and h/S/s/f/F/f*/B/B*/b/b*/n/W/W*. q/Q, cm and path operations operate outside BT; text and graphics
states must balance. Unknown operators, inline images, marked content/ActualText,
compatibility sections and invisible/clipping text modes refuse. Unsupported or
malformed content on any page fails the whole call; no earlier page escapes as
partial success. The adapter does not execute PDF scripts, actions or external
resources, and does not call a provider or tablet.

## Resource contract

| Stage | Bound |
| --- | --- |
| Original acquisition |16MiB plus one refusal sentinel;4096 Read calls |
| Eager loader | strict loading;1MiB decoded output per object/xref stream |
| Parsed objects |16384 retained objects, checked after parsing |
| Page tree |256 pages,1024 nodes,32 levels; reject cycles/duplicates/missing nodes and incorrect parent/count metadata |
| Content |64 streams/page;1MiB decoded page bytes and8MiB aggregate, reserved before decoding/concatenation |
| Content AST |32768 operations/page and131072/document, checked after strict AST materialization |
| Resources/state |64 fonts and64 images/page; aliases128bytes;32 indirect references and graphics stack entries |
| Normalized text |512KiB/page and4MiB/document UTF8, reserved before each append |

These are acquisition, per-stream decoding, adapter-work and output controls.
The maintained PDF loader/content parser allocates internally before the object
and operation eligibility checks. Many eager streams can cumulatively allocate
more than one stream allowance. These limits are **not hard process-wide RAM or
CPU bounds**, a parser sandbox or a blocking-reader timeout. Use an appropriate
process boundary if hostile-input containment is required by a caller.

All encrypted inputs refuse, whether password-required or empty-password. The
library can try an empty password internally; the adapter supplies/discovers no
credentials and checks retained encryption declaration and recorded encryption
history before accepting any pages.

## Development ingestion

```sh
cargo run --example normalize_pdf_source -- DOCUMENT_UUID document.pdf > normalized.json
```

The example accepts an explicitly supplied regular file and prints bounded JSON
only after complete success. It does not install a cache, bind a conversation,
select a source, navigate a tablet or call a model. Build `Index` from the returned
document and retain its exact key; original-byte, extractor or normalized changes
remain stale under existing index/cache checks. Run authored offline regressions:

```sh
cargo test --locked --test pdf_normalization
```

Central capability: `pdf-source-normalization` in ReMarkableBuddiesDocs, change
`normalize-pdf-source`. Broader retrieval, Reader integration and native acceptance
remain separate unfinished work.
