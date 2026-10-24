# Offline PDF text normalization

`retrieval::pdf::normalize(document_uuid, reader)` accepts caller-supplied bytes
and returns `PdfText { document, textless_pages }`. The document uses the existing
[normalized index](source-document-retrieval.md) and [owned cache](normalized-source-cache.md)
contracts. It hashes exact original bytes and records extractor revision
`pdf-explicit-unicode-r2-lopdf-0.46.0`. Every page stays in declared page-tree order,
including blank/image-only units; `page` is a one-based ordinal, not a printed
label or the tablet's current page. Entirely textless documents refuse.

## Supported profile

This profile uses pinned MIT-licensed lopdf 0.46.0, without its default
parallel/clock features. It admits standard Latin Type1 Courier, Helvetica and
Times families with explicit StandardEncoding, WinAnsiEncoding or MacRomanEncoding.
Supported accented characters and ligatures become Unicode; unmapped bytes refuse.
There is no guessed encoding, synthesized replacement character, OCR or font
fallback. This simple-font branch still refuses ToUnicode, FontDescriptor,
DescendantFonts, CharProcs and custom Differences before font-map decoding.

The additional declared Unicode branch admits indirect Type0 fonts with
Encoding=Identity-H, exactly one indirect CIDFontType2 descendant, matching
BaseFont/descriptor FontName and an explicit indirect ToUnicode stream. Font and
descriptor fields follow a closed typed metadata profile; unsupported vertical,
CFF/Type3, inherited/predefined maps or unknown fields refuse. Descriptor font
programs, CIDSet and stream-valued CIDToGIDMap are opaque input-owned bytes, with
external-file references refused. They are never interpreted, rendered or used
for system-font lookup. Success validates the declared text layer, not a valid
TrueType program or the appearance/visibility of its glyphs.

The ToUnicode profile requires a complete ordinary CMapType2 wrapper and bounded
CIDSystemInfo/CMapName metadata. Its source codes and codespace endpoints are
exactly two decoded hexadecimal bytes. It supports bfchar and bfrange with either
explicit target arrays or a single two-byte BMP increment without overflow or
surrogate crossing. Individual targets contain 2–16 even UTF16-BE bytes with valid
pairing; supplementary characters and multi-scalar ligatures use bfchar/arrays.
Unknown codes, odd/truncated shown strings, invalid targets, U+FFFD, overlapping
entries/codespaces, mismatched counts, usecmap and unknown/vertical operations
refuse the whole document. No earlier page escapes after a later failure.

The same maintained strict typed operation parser handles bounded map syntax.
The adapter validates map semantics and reserves each range before expansion;
it never invokes the upstream eager reverse-map constructor or replacement decoder.
Typed parsing normalizes odd hex spelling (`<001>` and `<0010>` both yield0010)
and duplicate raw dictionary keys to their last value. This profile uses those
parsed values and rejects duplicate metadata definition operations/mapping entries;
it does not claim to recover raw spelling or dictionary-key uniqueness. No extra
lexer or PostScript interpreter is used. Many font/content combinations remain
unsupported; this is not general PDF rendering or arbitrary-font coverage.

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
| Declared Unicode resources |64 distinct font references and64 distinct map references/document; validated immutable references are reused across pages/aliases |
| Decoded maps |64KiB/map and1MiB/document, remaining allowance passed before decode |
| Map AST |4096 operations/map and16384/document after strict AST materialization |
| Mapping expansion |4096 entries/map and65536/document, entire range reserved before expansion |
| Map target text |16 UTF16-BE bytes/entry and256KiB cumulative UTF8 table text/document before append |
| Unicode metadata |8192 width-array items/font and65536 visited typed metadata items/document; width ranges are not expanded |
| Normalized text |512KiB/page and4MiB/document UTF8, reserved before each append |

These are acquisition, per-stream decoding, adapter-work and output controls.
The maintained PDF loader/content parser allocates internally before the object
and operation eligibility checks. Many eager streams can cumulatively allocate
more than one stream allowance. Map AST materialization also precedes its operation
eligibility check. These limits are **not hard process-wide RAM or
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

The Unicode extension is the central change `normalize-unicode-pdf-source`.
Its new extractor revision applies to fresh simple-only PDFs too; old r1 derived
entries are not upgraded or relabeled as fresh. Original/extractor/normalized
changes continue to invalidate old expected index/cache keys without a schema
change. Authored fixtures include mixed scripts, ligatures/surrogates, exact UTF8
Index ranges, per-document reuse, decoded/map/metadata/text limits, malformed and
amplified declarations, later-page refusal and actual cache invalidation. These
are parser/data controls, not live-model, visual or native qualification.
