//! Authored PDF fixtures through the maintained writer; no personal documents.
#[path = "pdf_normalization/unicode.rs"]
mod unicode;
use lopdf::{
    dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions, Stream,
};
use remarkable_reader_buddy::{
    retrieval::{disk::DiskCache, pdf, Index, SourceFormat},
    storage::{digest, Store, StorePaths, Uuid},
};
use std::io::{self, Read};

struct Book {
    doc: Document,
    root: ObjectId,
    pages: Vec<ObjectId>,
    streams: Vec<ObjectId>,
    font: ObjectId,
}
impl Book {
    fn new(content: &[&[u8]]) -> Self {
        let mut doc = Document::with_version("1.5");
        let root = doc.new_object_id();
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
        let mut pages = Vec::new();
        let mut streams = Vec::new();
        for text in content {
            let mut stream = Stream::new(dictionary! {}, text.to_vec());
            stream.compress().unwrap();
            let stream = doc.add_object(stream);
            streams.push(stream);
            pages.push(doc.add_object(dictionary! { "Type" => "Page", "Parent" => root, "Contents" => stream, "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()] }));
        }
        doc.objects.insert(
            root,
            dictionary! { "Type" => "Pages", "Count" => pages.len() as i64,
            "Kids" => pages.iter().copied().map(Object::Reference).collect::<Vec<_>>(),
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } }
            .into(),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
        doc.trailer.set("Root", catalog);
        Self {
            doc,
            root,
            pages,
            streams,
            font,
        }
    }
    fn bytes(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.doc.save_to(&mut bytes).unwrap();
        bytes
    }
    fn refused(&mut self) -> String {
        pdf::normalize(Uuid::from_u128(1), self.bytes().as_slice())
            .unwrap_err()
            .to_string()
    }
}
const ALPHA: &[u8] = b"BT /F1 12 Tf (alpha acceleration) Tj ET";
fn found(index: &Index, query: &str) -> u32 {
    let result = index.query(index.key(), query, 2, 4096, false).unwrap();
    assert_eq!(result.passages.len(), 1, "query {query}");
    result.passages[0].page.unwrap()
}

#[test]
fn declared_page_order_blank_and_image_pages_have_real_query_provenance() {
    let mut book = Book::new(&[ALPHA, b"", b"/Im1 Do", b"BT /F1 12 Tf (omega orbit) Tj ET"]);
    let image = book.doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1, "BitsPerComponent" => 8, "ColorSpace" => "DeviceGray" }, vec![255]));
    book.doc
        .get_dictionary_mut(book.root)
        .unwrap()
        .get_mut(b"Resources")
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("XObject", dictionary! { "Im1" => image });
    book.doc.get_dictionary_mut(book.root).unwrap().set(
        "Kids",
        vec![
            book.pages[3].into(),
            book.pages[1].into(),
            book.pages[2].into(),
            book.pages[0].into(),
        ],
    );
    let bytes = book.bytes();
    let normalized = pdf::normalize(Uuid::from_u128(1), bytes.as_slice()).unwrap();
    assert_eq!(normalized.document.source_sha256, digest(&bytes));
    assert_eq!(normalized.document.format, SourceFormat::Pdf);
    assert_eq!(
        normalized.document.extractor_revision,
        pdf::EXTRACTOR_REVISION
    );
    assert_eq!(normalized.textless_pages, [2, 3]);
    assert_eq!(normalized.document.units.len(), 4);
    assert!(normalized.document.units[1].text.is_empty());
    let index = Index::build(normalized.document).unwrap();
    assert_eq!(found(&index, "orbit"), 1);
    assert_eq!(found(&index, "acceleration"), 4);
}

#[test]
fn encoding_unicode_inline_join_and_explicit_boundaries_are_searchable() {
    for (encoding, byte) in [("WinAnsiEncoding", 0xe9), ("MacRomanEncoding", 0x8e)] {
        let mut content = b"BT /F1 12 Tf (caf".to_vec();
        content.push(byte);
        content.extend_from_slice(b") Tj T* [(over) 0 (lap)] TJ (alpha) ' 0 0 (beta ) \" [(gamma) -150 (delta)] TJ ET BT /F1 12 Tf (epsilon) Tj ET");
        let mut book = Book::new(&[&content]);
        book.doc
            .get_dictionary_mut(book.font)
            .unwrap()
            .set("Encoding", encoding);
        let normalized = pdf::normalize(Uuid::from_u128(1), book.bytes().as_slice()).unwrap();
        assert_eq!(
            normalized.document.units[0].text,
            "café\noverlap\nalpha\nbeta gamma delta\nepsilon"
        );
        let index = Index::build(normalized.document).unwrap();
        for word in [
            "café", "overlap", "alpha", "beta", "gamma", "delta", "epsilon",
        ] {
            assert_eq!(found(&index, word), 1);
        }
    }
    let mut book = Book::new(&[b"BT /F1 12 Tf (\xae) Tj ET"]);
    book.doc
        .get_dictionary_mut(book.font)
        .unwrap()
        .set("Encoding", "StandardEncoding");
    assert_eq!(
        pdf::normalize(Uuid::from_u128(1), book.bytes().as_slice())
            .unwrap()
            .document
            .units[0]
            .text,
        "ﬁ"
    );
}

#[test]
fn actual_cache_accepts_pdf_and_refuses_changed_original_and_extractor_keys() {
    let root = std::env::temp_dir().join(format!("buddy-pdf-cache-{}", Uuid::new_v4()));
    let store = Store::open(StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    })
    .unwrap();
    let cache = DiskCache::open(&store).unwrap();
    let mut book = Book::new(&[ALPHA]);
    let bytes = book.bytes();
    let original = pdf::normalize(Uuid::from_u128(1), bytes.as_slice())
        .unwrap()
        .document;
    let key = cache.install(original.clone()).unwrap();
    assert_eq!(
        found(&cache.load(&key).unwrap().unwrap(), "acceleration"),
        1
    );
    let mut changed = bytes;
    changed.extend_from_slice(b"\n% authored metadata change\n");
    let changed = pdf::normalize(Uuid::from_u128(1), changed.as_slice())
        .unwrap()
        .document;
    assert_ne!(changed.source_sha256, original.source_sha256);
    cache.install(changed).unwrap();
    assert!(cache.load(&key).is_err());
    let key = cache.install(original.clone()).unwrap();
    let mut revised = original;
    revised.extractor_revision.push_str("-next");
    cache.install(revised).unwrap();
    assert!(cache.load(&key).is_err());
    drop(cache);
    drop(store);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    assert!(root
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("buddy-pdf-cache-"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_fonts_and_huge_cmap_refuse_before_mapping_expansion() {
    for key in [
        "ToUnicode",
        "FontDescriptor",
        "DescendantFonts",
        "CharProcs",
    ] {
        let mut book = Book::new(&[ALPHA]);
        let map = book.doc.add_object(Stream::new(
            dictionary! {},
            b"1 beginbfrange <00000000> <ffffffff> <0041> endbfrange".to_vec(),
        ));
        book.doc
            .get_dictionary_mut(book.font)
            .unwrap()
            .set(key, map);
        assert_eq!(book.refused(), "Unsupported PDF resource");
    }
    for (key, value) in [
        ("Subtype", "Type0"),
        ("Subtype", "Type3"),
        ("BaseFont", "Symbol"),
        ("Encoding", "Identity-H"),
        ("Encoding", "UnknownEncoding"),
    ] {
        let mut book = Book::new(&[ALPHA]);
        book.doc
            .get_dictionary_mut(book.font)
            .unwrap()
            .set(key, value);
        assert!(!book.refused().is_empty());
    }
    let mut book = Book::new(&[ALPHA]);
    book.doc.get_dictionary_mut(book.font).unwrap().set("Encoding", dictionary! { "BaseEncoding" => "WinAnsiEncoding", "Differences" => vec![65.into(), Object::Name(b"B".to_vec())] });
    assert_eq!(book.refused(), "Unsupported PDF font encoding");
}

#[test]
fn missing_fonts_unmapped_glyphs_and_later_page_unsupported_text_never_return_partial() {
    for bad in [
        b"BT (missing font) Tj ET".as_slice(),
        b"BT /Missing 12 Tf (x) Tj ET",
        b"BT /F1 12 Tf (\0) Tj ET",
        b"BT /F1 12 Tf (alpha) Tj 10 0 Td (beta) Tj ET",
        b"/Span << /ActualText (different) >> BDC BT /F1 12 Tf (x) Tj ET EMC",
        b"BT /F1 12 Tf (x) Tj ET @@@",
        b"BT /F1 12 Tf 3 Tr (hidden) Tj ET",
        b"BT /F1 12 Tf (unfinished) Tj",
        b"Q",
        b"BX EX",
        b"BT /F1 12 Tf 0 0 m (invalid path state) Tj ET",
    ] {
        let mut book = Book::new(&[ALPHA, bad]);
        assert!(!book.refused().is_empty());
    }
    let mut book = Book::new(&[ALPHA, b"/Form Do"]);
    let form = book.doc.add_object(Stream::new(
        dictionary! { "Subtype" => "Form", "BBox" => vec![0.into(),0.into(),20.into(),20.into()] },
        ALPHA.to_vec(),
    ));
    book.doc
        .get_dictionary_mut(book.root)
        .unwrap()
        .get_mut(b"Resources")
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("XObject", dictionary! { "Form" => form });
    assert_eq!(book.refused(), "Unsupported PDF XObject");
}

#[test]
fn encrypted_empty_password_and_password_required_sources_both_refuse() {
    for password in ["", "authored-test-password"] {
        let mut book = Book::new(&[ALPHA]);
        book.doc.trailer.set(
            "ID",
            vec![
                Object::string_literal(vec![1; 16]),
                Object::string_literal(vec![2; 16]),
            ],
        );
        let state = EncryptionState::try_from(EncryptionVersion::V1 {
            document: &book.doc,
            owner_password: "authored-owner",
            user_password: password,
            permissions: Permissions::all(),
        })
        .unwrap();
        book.doc.encrypt(&state).unwrap();
        assert_eq!(book.refused(), "Encrypted PDF unsupported");
    }
    let mut book = Book::new(&[ALPHA]);
    book.doc.trailer.set("Encrypt", Object::Null);
    assert!(!book.refused().is_empty());
}

#[test]
fn malformed_page_tree_and_textless_sources_refuse() {
    let mut empty = Book::new(&[b""]);
    assert_eq!(empty.refused(), "PDF has no supported text");
    let mut count = Book::new(&[ALPHA]);
    count
        .doc
        .get_dictionary_mut(count.root)
        .unwrap()
        .set("Count", 2);
    assert_eq!(count.refused(), "Inconsistent PDF page count");
    let mut duplicate = Book::new(&[ALPHA]);
    duplicate
        .doc
        .get_dictionary_mut(duplicate.root)
        .unwrap()
        .set(
            "Kids",
            vec![duplicate.pages[0].into(), duplicate.pages[0].into()],
        );
    assert_eq!(duplicate.refused(), "PDF page tree bound or cycle");
    let mut parent = Book::new(&[ALPHA]);
    parent
        .doc
        .get_dictionary_mut(parent.pages[0])
        .unwrap()
        .set("Parent", (900, 0));
    assert_eq!(parent.refused(), "Invalid PDF page parent");
    let mut missing = Book::new(&[ALPHA]);
    missing
        .doc
        .get_dictionary_mut(missing.root)
        .unwrap()
        .set("Kids", vec![Object::Reference((999, 0))]);
    assert_eq!(missing.refused(), "Missing PDF page node");
    assert!(pdf::normalize(Uuid::from_u128(1), b"not a PDF".as_slice()).is_err());
}

#[test]
fn content_filters_external_resources_and_nonpage_text_are_explicitly_unsupported() {
    for (key, value) in [
        ("Filter", Object::Name(b"UnknownFilter".to_vec())),
        ("Filter", Object::Integer(1)),
        ("DecodeParms", dictionary! {}.into()),
        (
            "F",
            Object::string_literal("https://invalid.example/never-fetched"),
        ),
    ] {
        let mut book = Book::new(&[ALPHA]);
        book.doc
            .get_object_mut(book.streams[0])
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .dict
            .set(key, value);
        assert!(!book.refused().is_empty());
    }
    let mut book = Book::new(&[ALPHA]);
    book.doc
        .get_dictionary_mut(book.pages[0])
        .unwrap()
        .set("Annots", vec![Object::Null]);
    assert_eq!(book.refused(), "Unsupported PDF resource");
    let mut book = Book::new(&[ALPHA]);
    book.doc
        .catalog_mut()
        .unwrap()
        .set("AcroForm", dictionary! {});
    assert_eq!(book.refused(), "Unsupported PDF resource");
}

#[test]
fn original_acquisition_sentinel_calls_and_identity_are_bounded() {
    struct Sentinel {
        read: usize,
    }
    impl Read for Sentinel {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            out.fill(b'x');
            self.read += out.len();
            Ok(out.len())
        }
    }
    let mut source = Sentinel { read: 0 };
    assert_eq!(
        pdf::normalize(Uuid::from_u128(1), &mut source)
            .unwrap_err()
            .to_string(),
        "PDF original byte bound"
    );
    assert_eq!(source.read, 16 * 1024 * 1024 + 1);
    struct Slow(usize);
    impl Read for Slow {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.0 += 1;
            out[0] = b'x';
            Ok(1)
        }
    }
    let mut source = Slow(0);
    assert!(pdf::normalize(Uuid::from_u128(1), &mut source).is_err());
    assert_eq!(source.0, 4096);
    let mut source = Slow(0);
    assert!(pdf::normalize(Uuid::nil(), &mut source).is_err());
    assert_eq!(source.0, 0);
}

#[test]
fn compressed_content_page_and_aggregate_decode_limits_apply_before_text_collection() {
    let mut huge = vec![b' '; 1024 * 1024 + 1];
    huge.extend_from_slice(ALPHA);
    let mut book = Book::new(&[&huge]);
    assert!(book
        .doc
        .get_object(book.streams[0])
        .unwrap()
        .as_stream()
        .unwrap()
        .dict
        .has(b"Filter"));
    assert_eq!(book.refused(), "PDF stream decode failed or exhausted");
    let mut almost = vec![b' '; 1024 * 1024 - 100];
    almost.extend_from_slice(ALPHA);
    let content: Vec<&[u8]> = vec![almost.as_slice(); 9];
    let mut book = Book::new(&content);
    assert_eq!(book.refused(), "PDF stream decode failed or exhausted");
}

#[test]
fn text_operation_graphics_and_page_limits_refuse() {
    let large_text = format!("BT /F1 12 Tf ({}) Tj ET", "a".repeat(512 * 1024 + 1));
    assert_eq!(
        Book::new(&[large_text.as_bytes()]).refused(),
        "PDF text byte bound"
    );
    let text = format!("BT /F1 12 Tf ({}) Tj ET", "a".repeat(500 * 1024));
    assert_eq!(
        Book::new(&[text.as_bytes(); 9]).refused(),
        "PDF text byte bound"
    );
    let ops = format!(
        "{}{}",
        "q Q ".repeat(16385),
        std::str::from_utf8(ALPHA).unwrap()
    );
    assert_eq!(
        Book::new(&[ops.as_bytes()]).refused(),
        "PDF operation eligibility bound"
    );
    let ops = format!(
        "{}{}",
        "q Q ".repeat(8000),
        std::str::from_utf8(ALPHA).unwrap()
    );
    assert_eq!(
        Book::new(&[ops.as_bytes(); 9]).refused(),
        "PDF operation eligibility bound"
    );
    let state = format!("{}{}", "q ".repeat(33), std::str::from_utf8(ALPHA).unwrap());
    assert_eq!(
        Book::new(&[state.as_bytes()]).refused(),
        "PDF graphics state bound"
    );
    assert_eq!(
        Book::new(&vec![ALPHA; 257]).refused(),
        "PDF page count bound"
    );
}

#[test]
fn tree_reference_resource_and_stream_work_bounds_refuse() {
    let mut deep = Book::new(&[ALPHA]);
    let mut parent = deep.root;
    for _ in 0..32 {
        let child = deep
            .doc
            .add_object(dictionary! { "Type" => "Pages", "Parent" => parent, "Count" => 1 });
        deep.doc
            .get_dictionary_mut(parent)
            .unwrap()
            .set("Kids", vec![Object::Reference(child)]);
        parent = child;
    }
    deep.doc
        .get_dictionary_mut(parent)
        .unwrap()
        .set("Kids", vec![Object::Reference(deep.pages[0])]);
    deep.doc
        .get_dictionary_mut(deep.pages[0])
        .unwrap()
        .set("Parent", parent);
    assert_eq!(deep.refused(), "PDF page tree bound or cycle");

    let mut refs = Book::new(&[ALPHA]);
    let mut reference = Object::Reference(refs.streams[0]);
    for _ in 0..33 {
        reference = Object::Reference(refs.doc.add_object(reference));
    }
    refs.doc
        .get_dictionary_mut(refs.pages[0])
        .unwrap()
        .set("Contents", reference);
    assert_eq!(refs.refused(), "PDF reference limit or cycle");

    let mut cycle = Book::new(&[ALPHA]);
    let object = cycle.doc.new_object_id();
    cycle.doc.objects.insert(object, Object::Reference(object));
    cycle
        .doc
        .get_dictionary_mut(cycle.pages[0])
        .unwrap()
        .set("Contents", object);
    assert_eq!(cycle.refused(), "PDF reference limit or cycle");

    let mut catalog = Book::new(&[ALPHA]);
    let mut reference = catalog.doc.trailer.get(b"Root").unwrap().clone();
    for _ in 0..33 {
        reference = Object::Reference(catalog.doc.add_object(reference));
    }
    catalog.doc.trailer.set("Root", reference);
    assert_eq!(catalog.refused(), "PDF reference limit or cycle");

    let mut streams = Book::new(&[ALPHA]);
    streams
        .doc
        .get_dictionary_mut(streams.pages[0])
        .unwrap()
        .set("Contents", vec![Object::Reference(streams.streams[0]); 65]);
    assert_eq!(streams.refused(), "PDF content stream count bound");

    let mut fonts = Book::new(&[ALPHA]);
    let mut entries = lopdf::Dictionary::new();
    for n in 0..65 {
        entries.set(format!("Font{n}"), fonts.font);
    }
    fonts
        .doc
        .get_dictionary_mut(fonts.root)
        .unwrap()
        .get_mut(b"Resources")
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Font", entries);
    assert_eq!(fonts.refused(), "PDF font resource bound");

    let mut aliases = Book::new(&[ALPHA]);
    let mut entries = lopdf::Dictionary::new();
    entries.set("a".repeat(129), aliases.font);
    aliases
        .doc
        .get_dictionary_mut(aliases.root)
        .unwrap()
        .get_mut(b"Resources")
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Font", entries);
    assert_eq!(aliases.refused(), "PDF font alias bound");
}

#[test]
fn eager_object_stream_and_postparse_object_limits_remain_distinct() {
    let mut bomb = Book::new(&[ALPHA]);
    // The plain writer drops existing ObjStm containers. Ask its modern writer
    // to create a real compressed object stream containing a large object.
    bomb.doc
        .add_object(Object::string_literal(vec![b'a'; 1024 * 1024 + 1]));
    let mut bytes = Vec::new();
    bomb.doc.save_modern(&mut bytes).unwrap();
    assert!(bytes
        .windows(b"ObjStm".len())
        .any(|value| value == b"ObjStm"));
    assert_eq!(
        pdf::normalize(Uuid::from_u128(1), bytes.as_slice())
            .unwrap_err()
            .to_string(),
        "PDF load failed or exhausted"
    );
    let mut objects = Book::new(&[ALPHA]);
    for _ in 0..16384 {
        objects.doc.add_object(dictionary! {});
    }
    assert_eq!(objects.refused(), "PDF object eligibility bound");
}

#[test]
fn page_tree_node_budget_is_independent_of_page_and_depth_budgets() {
    let mut tree = Book::new(&[ALPHA; 256]);
    let mut tops = Vec::new();
    for page in tree.pages.clone() {
        let mut child = page;
        for _ in 0..3 {
            let node = tree.doc.add_object(dictionary! { "Type" => "Pages", "Count" => 1, "Kids" => vec![Object::Reference(child)] });
            tree.doc
                .get_dictionary_mut(child)
                .unwrap()
                .set("Parent", node);
            child = node;
        }
        tree.doc
            .get_dictionary_mut(child)
            .unwrap()
            .set("Parent", tree.root);
        tops.push(Object::Reference(child));
    }
    tree.doc
        .get_dictionary_mut(tree.root)
        .unwrap()
        .set("Kids", tops);
    assert_eq!(tree.refused(), "PDF page tree bound or cycle");
}
