use super::*;
use lopdf::Dictionary;
use std::path::PathBuf;

const SHOWN: &[u8] = b"BT /U0 12 Tf <0001> Tj ET";
const ONE: &str = "1 beginbfchar <0001> <03B1> endbfchar";
fn cmap(body: &str) -> Vec<u8> {
    format!("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /FixtureMap def\n/CMapType 2 def\n1 begincodespacerange <0000> <ffff> endcodespacerange\n{body}\nendcmap\nCMapName currentdict /CMap defineresource pop\nend end\n").into_bytes()
}
fn font_table(book: &mut Book) -> &mut Dictionary {
    book.doc
        .get_dictionary_mut(book.root)
        .unwrap()
        .get_mut(b"Resources")
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .get_mut(b"Font")
        .unwrap()
        .as_dict_mut()
        .unwrap()
}
fn clear_fonts(book: &mut Book) {
    book.doc
        .get_dictionary_mut(book.root)
        .unwrap()
        .get_mut(b"Resources")
        .unwrap()
        .as_dict_mut()
        .unwrap()
        .set("Font", Dictionary::new());
}
struct Ids {
    font: ObjectId,
    descendant: ObjectId,
    descriptor: ObjectId,
    map: ObjectId,
}
fn add_map(book: &mut Book, bytes: Vec<u8>, compressed: bool) -> ObjectId {
    let mut stream = Stream::new(dictionary! {}, bytes);
    if compressed {
        stream.compress().unwrap();
    }
    book.doc.add_object(stream)
}
fn add_font(book: &mut Book, map: ObjectId, alias: &str) -> Ids {
    let descriptor = book.doc.add_object(
        dictionary! { "Type" => "FontDescriptor", "FontName" => "FixtureUnicode",
        "Flags" => 4, "FontBBox" => vec![0.into(), 0.into(), 1000.into(), 1000.into()],
        "ItalicAngle" => 0, "Ascent" => 800, "Descent" => -200, "CapHeight" => 700, "StemV" => 80 },
    );
    let descendant = book.doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "FixtureUnicode",
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "FontDescriptor" => descriptor, "DW" => 1000, "W" => vec![1.into(), Object::Array(vec![500.into(), 600.into()]), 3.into(), 4.into(), 700.into()], "CIDToGIDMap" => "Identity" });
    let font = book.doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "FixtureUnicode", "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(descendant)], "ToUnicode" => map });
    font_table(book).set(alias, font);
    Ids {
        font,
        descendant,
        descriptor,
        map,
    }
}
fn unicode_book(content: &[&[u8]], body: &str) -> (Book, Ids) {
    let mut book = Book::new(content);
    let map = add_map(&mut book, cmap(body), true);
    let ids = add_font(&mut book, map, "U0");
    (book, ids)
}
fn normalize(book: &mut Book) -> pdf::PdfText {
    pdf::normalize(Uuid::from_u128(1), book.bytes().as_slice()).unwrap()
}
fn many_targets(count: usize, target: &str) -> String {
    let mut out = String::new();
    for start in (1..=count).step_by(100) {
        let end = (start + 99).min(count);
        out.push_str(&format!("{} beginbfchar\n", end - start + 1));
        for code in start..=end {
            out.push_str(&format!("<{code:04x}> <{target}>\n"));
        }
        out.push_str("endbfchar\n");
    }
    out
}

#[test]
fn declared_unicode_mixed_pages_ranges_ligatures_and_supplementary_text_reach_actual_index() {
    let body = "4 beginbfchar <0001> <03b1> <0002> <0020> <0003> <006f0072006200690074> <0004> <00660069> endbfchar
        1 beginbfchar <0005> <d83dde00> endbfchar
        1 beginbfrange <0006> <0007> <0041> endbfrange
        1 beginbfrange <0008> <0009> [<03b2> <03b3>] endbfrange";
    let (mut book, _) = unicode_book(
        &[
            ALPHA,
            b"",
            b"BT /U0 12 Tf <0001000200030002000400020005000200060007000200080009> Tj ET",
        ],
        body,
    );
    book.doc.get_dictionary_mut(book.root).unwrap().set(
        "Kids",
        vec![
            book.pages[2].into(),
            book.pages[1].into(),
            book.pages[0].into(),
        ],
    );
    let bytes = book.bytes();
    let normalized = pdf::normalize(Uuid::from_u128(1), bytes.as_slice()).unwrap();
    assert_eq!(normalized.document.source_sha256, digest(&bytes));
    assert_eq!(
        normalized.document.extractor_revision,
        "pdf-explicit-unicode-r2-lopdf-0.46.0"
    );
    assert_eq!(normalized.document.units[0].text, "α orbit fi 😀 AB βγ");
    assert_eq!(normalized.document.units[2].text, "alpha acceleration");
    assert_eq!(normalized.textless_pages, [2]);
    let index = Index::build(normalized.document).unwrap();
    for query in ["α", "orbit", "fi", "AB", "βγ"] {
        assert_eq!(found(&index, query), 1);
    }
    assert_eq!(found(&index, "acceleration"), 3);
    let result = index.query(index.key(), "orbit", 2, 4096, false).unwrap();
    let passage = &result.passages[0];
    assert_eq!(passage.range, [0, passage.text.len()]);
    assert_eq!(passage.text, "α orbit fi 😀 AB βγ");
}

#[test]
fn immutable_reference_reuse_handles_many_pages_aliases_and_same_name_distinct_maps() {
    let pages = vec![SHOWN; 256];
    let (mut book, ids) = unicode_book(&pages, ONE);
    let mut map = cmap(ONE);
    map.resize(64 * 1024, b' ');
    book.doc
        .objects
        .insert(ids.map, Stream::new(dictionary! {}, map).into());
    book.doc
        .get_dictionary_mut(ids.descendant)
        .unwrap()
        .set("W", vec![0.into(), Object::Array(vec![500.into(); 8000])]);
    for n in 1..8 {
        font_table(&mut book).set(format!("U{n}"), ids.font);
    }
    let result = normalize(&mut book);
    assert_eq!(result.document.units.len(), 256);
    assert!(result.document.units.iter().all(|u| u.text == "α"));
    let (mut book, _) = unicode_book(&[b"BT /U0 12 Tf <0001> Tj /U1 12 Tf <0001> Tj ET"], ONE);
    let second = add_map(
        &mut book,
        cmap("1 beginbfchar <0001> <03b2> endbfchar"),
        false,
    );
    add_font(&mut book, second, "U1");
    assert_eq!(normalize(&mut book).document.units[0].text, "αβ");
}

#[test]
fn font_reference_bound_and_shared_map_reuse_are_actual_document_controls() {
    let mut book = Book::new(&[SHOWN, SHOWN]);
    clear_fonts(&mut book);
    let mut data = cmap(ONE);
    data.resize(64 * 1024, b' ');
    let map = add_map(&mut book, data, false);
    for n in 0..64 {
        add_font(&mut book, map, &format!("U{n}"));
    }
    assert_eq!(normalize(&mut book).document.units[0].text, "α");
    // Give the extra font its own page resource set, preserving the 64/page cap.
    let extra = add_font(&mut book, map, "extra");
    font_table(&mut book).remove(b"extra");
    book.doc.get_dictionary_mut(book.pages[1]).unwrap().set(
        "Resources",
        dictionary! { "Font" => dictionary! { "U0" => extra.font } },
    );
    assert_eq!(book.refused(), "PDF Unicode font count bound");
}

#[test]
fn sixty_four_distinct_map_and_font_references_do_not_alias_by_declared_names() {
    let mut book = Book::new(&[b"BT /U63 12 Tf <0001> Tj ET"]);
    clear_fonts(&mut book);
    for n in 0..64 {
        let body = if n == 63 {
            "1 beginbfchar <0001> <03b2> endbfchar"
        } else {
            ONE
        };
        let map = add_map(&mut book, cmap(body), false);
        add_font(&mut book, map, &format!("U{n}"));
    }
    assert_eq!(normalize(&mut book).document.units[0].text, "β");
}

#[test]
fn declared_mapping_expansion_is_reserved_before_per_map_and_document_limits() {
    let range = "1 beginbfrange <0000> <0fff> <4e00> endbfrange";
    let (mut book, _) = unicode_book(&[SHOWN], range);
    assert_eq!(normalize(&mut book).document.units[0].text, "丁");
    let (mut too_many, _) = unicode_book(
        &[ALPHA, SHOWN],
        "1 beginbfrange <0000> <1000> <4e00> endbfrange",
    );
    assert_eq!(too_many.refused(), "PDF Unicode mapping entry bound");
    let (mut huge, _) = unicode_book(
        &[ALPHA, SHOWN],
        "1 beginbfrange <00000000> <ffffffff> <0041> endbfrange",
    );
    assert_eq!(huge.refused(), "Unsupported PDF Unicode code width");
    let mut book = Book::new(&[SHOWN]);
    clear_fonts(&mut book);
    for n in 0..16 {
        let map = add_map(&mut book, cmap(range), false);
        add_font(&mut book, map, &format!("U{n}"));
    }
    assert_eq!(normalize(&mut book).document.units[0].text, "丁");
    let map = add_map(&mut book, cmap(ONE), false);
    add_font(&mut book, map, "last");
    assert_eq!(book.refused(), "PDF Unicode mapping entry bound");
}

#[test]
fn decoded_map_byte_limits_precede_typed_mapping_and_accumulate_only_new_references() {
    let mut book = Book::new(&[SHOWN]);
    clear_fonts(&mut book);
    for n in 0..16 {
        let mut data = cmap(ONE);
        data.resize(64 * 1024, b' ');
        let map = add_map(&mut book, data, true);
        add_font(&mut book, map, &format!("U{n}"));
    }
    assert_eq!(normalize(&mut book).document.units[0].text, "α");
    let map = add_map(&mut book, cmap(ONE), false);
    add_font(&mut book, map, "overflow");
    assert_eq!(book.refused(), "PDF cumulative CMap byte bound");
    let (mut one, ids) = unicode_book(&[SHOWN], ONE);
    let mut data = cmap(ONE);
    data.resize(64 * 1024 + 1, b' ');
    let mut stream = Stream::new(dictionary! {}, data);
    stream.compress().unwrap();
    one.doc.objects.insert(ids.map, stream.into());
    assert_eq!(one.refused(), "PDF stream decode failed or exhausted");
}

#[test]
fn utf16_target_and_cumulative_utf8_table_limits_have_exact_positive_boundaries() {
    let long = "6f226f226f226f226f226f226f226f22";
    let (mut book, _) = unicode_book(
        &[SHOWN],
        &format!("1 beginbfchar <0001> <{long}> endbfchar"),
    );
    assert_eq!(normalize(&mut book).document.units[0].text, "漢".repeat(8));
    let (mut too_long, _) = unicode_book(
        &[SHOWN],
        &format!("1 beginbfchar <0001> <{long}0041> endbfchar"),
    );
    assert_eq!(too_long.refused(), "PDF Unicode target byte bound");
    let mut book = Book::new(&[SHOWN]);
    clear_fonts(&mut book);
    for n in 0..10 {
        let map = add_map(&mut book, cmap(&many_targets(1000, long)), true);
        add_font(&mut book, map, &format!("U{n}"));
    }
    // 240000 + 922*24 + 2*8 = 262144 UTF8 map bytes exactly.
    let mut last = many_targets(922, long);
    last.push_str("2 beginbfchar <039b> <00410041004100410041004100410041> <039c> <00410041004100410041004100410041> endbfchar");
    let map = add_map(&mut book, cmap(&last), true);
    let ids = add_font(&mut book, map, "last");
    assert_eq!(normalize(&mut book).document.units[0].text, "漢".repeat(8));
    last.push_str("\n1 beginbfchar <039d> <0041> endbfchar");
    book.doc
        .objects
        .insert(ids.map, Stream::new(dictionary! {}, cmap(&last)).into());
    assert_eq!(book.refused(), "PDF Unicode target text bound");
}

#[test]
fn metadata_width_item_and_document_work_budgets_preserve_zero_expansion_ranges() {
    let (mut book, ids) = unicode_book(&[SHOWN], ONE);
    // Width ranges are checked metadata, never expanded into a width table.
    book.doc
        .get_dictionary_mut(ids.descendant)
        .unwrap()
        .set("W", vec![0.into(), 65535.into(), 500.into()]);
    assert_eq!(normalize(&mut book).document.units[0].text, "α");
    book.doc
        .get_dictionary_mut(ids.descendant)
        .unwrap()
        .set("W", vec![0.into(), Object::Array(vec![500.into(); 8190])]);
    assert_eq!(normalize(&mut book).document.units[0].text, "α");
    book.doc
        .get_dictionary_mut(ids.descendant)
        .unwrap()
        .set("W", vec![0.into(), Object::Array(vec![500.into(); 8191])]);
    assert_eq!(book.refused(), "PDF font width item bound");
    let mut book = Book::new(&[SHOWN]);
    clear_fonts(&mut book);
    let map = add_map(&mut book, cmap(ONE), true);
    for n in 0..9 {
        let ids = add_font(&mut book, map, &format!("U{n}"));
        book.doc
            .get_dictionary_mut(ids.descendant)
            .unwrap()
            .set("W", vec![0.into(), Object::Array(vec![500.into(); 8000])]);
    }
    assert_eq!(book.refused(), "PDF Unicode metadata work bound");
}

#[test]
fn malformed_ambiguous_inherited_or_unknown_mapping_semantics_never_return_partial_text() {
    let bodies = [
        "1 beginbfchar <0001> <d800> endbfchar",
        "1 beginbfchar <0001> <dc00> endbfchar",
        "1 beginbfchar <0001> <fffd> endbfchar",
        "1 beginbfchar <0001> <0000> endbfchar",
        "1 beginbfchar <0001> <> endbfchar",
        "1 beginbfchar <0001> <00> endbfchar",
        "1 beginbfchar <01> <0041> endbfchar",
        "1 beginbfchar (AB) <0041> endbfchar",
        "1 beginbfchar <0001> (AB) endbfchar",
        "2 beginbfchar <0001> <0041> endbfchar",
        "1 beginbfrange <0002> <0001> <0041> endbfrange",
        "1 beginbfrange <0001> <0002> [<0041>] endbfrange",
        "1 beginbfrange <0001> <0002> <ffff> endbfrange",
        "1 beginbfrange <0001> <0002> <d7ff> endbfrange",
        "1 beginbfrange <0001> <0002> <00410042> endbfrange",
        "2 beginbfchar <0001> <0041> <0001> <0041> endbfchar",
        "1 beginbfchar <0001> <0041> endbfchar 1 beginbfrange <0001> <0002> <0042> endbfrange",
        "/Parent usecmap",
        "1 beginbfchar <0001> <0041> endbfchar 1 begincodespacerange <0000> <ffff> endcodespacerange",
    ];
    for body in bodies {
        let (mut book, _) = unicode_book(&[ALPHA, SHOWN], body);
        book.doc.get_dictionary_mut(book.pages[0]).unwrap().set(
            "Resources",
            dictionary! { "Font" => dictionary! { "F1" => book.font } },
        );
        let error = pdf::normalize(Uuid::from_u128(1), book.bytes().as_slice()).unwrap_err();
        assert!(!format!("{error:#}").contains(body));
    }
    for (old, new) in [
        ("<0000> <ffff>", "<0002> <ffff>"),
        ("<0000> <ffff>", "<0000> <ffffffff>"),
        (
            "1 begincodespacerange <0000> <ffff>",
            "2 begincodespacerange <0000> <0010> <0010> <ffff>",
        ),
        ("/CMapType 2 def", "/CMapType 1 def"),
        ("/CMapType 2 def", "/CMapType 2 def /CMapType 2 def"),
        ("/CMapType 2 def", "/CMapType 2 def /WMode 1 def"),
        ("pop\nend end", "pop\nend"),
        ("12 dict", "65 dict"),
    ] {
        let (mut book, ids) = unicode_book(&[ALPHA, SHOWN], ONE);
        let text = String::from_utf8(cmap(ONE)).unwrap().replace(old, new);
        book.doc.objects.insert(
            ids.map,
            Stream::new(dictionary! {}, text.into_bytes()).into(),
        );
        assert!(!book.refused().is_empty(), "mutation {old}");
    }
}

#[test]
fn fixed_width_shown_strings_and_existing_content_state_refuse_late_failures() {
    for bad in [
        b"BT /U0 12 Tf <00> Tj ET".as_slice(),
        b"BT /U0 12 Tf <0002> Tj ET",
        b"BT /U0 12 Tf [<00> <01>] TJ ET",
        b"BT /U0 12 Tf <0001> Tj 2 0 Td <0001> Tj ET",
        b"BT /U0 12 Tf 3 Tr <0001> Tj ET",
        b"BT /U0 12 Tf <0001> Tj ET @@@",
    ] {
        let (mut book, _) = unicode_book(&[ALPHA, bad], ONE);
        assert!(pdf::normalize(Uuid::from_u128(1), book.bytes().as_slice()).is_err());
    }
    let (mut good, _) = unicode_book(
        &[b"BT /U0 12 Tf <0001> Tj T* [<0001> -150 <0001>] TJ <0001> ' 0 0 <0001> \" ET"],
        ONE,
    );
    assert_eq!(normalize(&mut good).document.units[0].text, "α\nα α\nα\nα");
}

#[test]
fn font_and_map_metadata_are_closed_and_external_or_inherited_resources_refuse() {
    for mode in 0..19 {
        let (mut book, ids) = unicode_book(&[ALPHA, SHOWN], ONE);
        match mode {
            0 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Encoding", "Identity-V"),
            1 => book
                .doc
                .get_dictionary_mut(ids.descendant)
                .unwrap()
                .set("Subtype", "CIDFontType0"),
            2 => book
                .doc
                .get_dictionary_mut(ids.descendant)
                .unwrap()
                .set("BaseFont", "other"),
            3 => book
                .doc
                .get_dictionary_mut(ids.descriptor)
                .unwrap()
                .set("FontName", "other"),
            4 => book.doc.get_dictionary_mut(ids.font).unwrap().set(
                "DescendantFonts",
                vec![ids.descendant.into(), ids.descendant.into()],
            ),
            5 => book
                .doc
                .get_dictionary_mut(ids.descendant)
                .unwrap()
                .set("W2", vec![1.into()]),
            6 => book
                .doc
                .get_dictionary_mut(ids.descriptor)
                .unwrap()
                .set("Flags", -1),
            7 => book
                .doc
                .get_dictionary_mut(ids.descriptor)
                .unwrap()
                .set("FontBBox", vec![0.into()]),
            8 => book.doc.get_dictionary_mut(ids.descendant).unwrap().set(
                "W",
                vec![65535.into(), Object::Array(vec![1.into(), 1.into()])],
            ),
            9 => book
                .doc
                .get_dictionary_mut(ids.descendant)
                .unwrap()
                .set("W", vec![1.into(), 0.into(), 500.into()]),
            10 => book
                .doc
                .get_dictionary_mut(ids.descendant)
                .unwrap()
                .set("CIDToGIDMap", "Unknown"),
            11 => book
                .doc
                .get_object_mut(ids.map)
                .unwrap()
                .as_stream_mut()
                .unwrap()
                .dict
                .set("UseCMap", "Inherited"),
            12 => book
                .doc
                .get_object_mut(ids.map)
                .unwrap()
                .as_stream_mut()
                .unwrap()
                .dict
                .set("F", Object::string_literal("private-source-sentinel")),
            13 => book
                .doc
                .get_object_mut(ids.map)
                .unwrap()
                .as_stream_mut()
                .unwrap()
                .dict
                .set("Filter", "Unknown"),
            14 => book
                .doc
                .get_object_mut(ids.map)
                .unwrap()
                .as_stream_mut()
                .unwrap()
                .dict
                .set("DecodeParms", Dictionary::new()),
            15 => book
                .doc
                .get_object_mut(ids.map)
                .unwrap()
                .as_stream_mut()
                .unwrap()
                .dict
                .set("CMapName", "Other"),
            16 => book
                .doc
                .get_object_mut(ids.map)
                .unwrap()
                .as_stream_mut()
                .unwrap()
                .dict
                .set("WMode", 1),
            17 => {
                let bad = book.doc.add_object(Stream::new(
                    dictionary! {"F"=>Object::string_literal("private-source-sentinel")},
                    vec![],
                ));
                book.doc
                    .get_dictionary_mut(ids.descriptor)
                    .unwrap()
                    .set("FontFile2", bad);
            }
            _ => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("ToUnicode", Object::Reference((999999, 0))),
        }
        let error = pdf::normalize(Uuid::from_u128(1), book.bytes().as_slice()).unwrap_err();
        assert!(!format!("{error:#}").contains("private-source-sentinel"));
    }
}

#[test]
fn typed_parser_normalization_and_opaque_programs_do_not_claim_raw_syntax_or_rendering() {
    let (mut book, ids) = unicode_book(&[SHOWN], "1 beginbfchar <001> <041> endbfchar");
    let data = String::from_utf8(cmap("1 beginbfchar <001> <041> endbfchar"))
        .unwrap()
        .replace(
            "/Registry (Adobe)",
            "/Registry (discarded) /Registry (Adobe)",
        );
    book.doc.objects.insert(
        ids.map,
        Stream::new(dictionary! {}, data.into_bytes()).into(),
    );
    let program = book.doc.add_object(Stream::new(
        dictionary! {},
        b"opaque authored bytes, not a qualified TrueType program".to_vec(),
    ));
    book.doc
        .get_dictionary_mut(ids.descriptor)
        .unwrap()
        .set("FontFile2", program);
    book.doc
        .get_dictionary_mut(ids.descriptor)
        .unwrap()
        .set("FontFamily", Object::string_literal(""));
    book.doc
        .get_dictionary_mut(ids.descriptor)
        .unwrap()
        .set("CharSet", Object::string_literal(""));
    // The padded source <001> denotes0010, not0001.
    book.doc
        .get_object_mut(book.streams[0])
        .unwrap()
        .as_stream_mut()
        .unwrap()
        .set_plain_content(b"BT /U0 12 Tf <0010> Tj ET".to_vec());
    assert_eq!(normalize(&mut book).document.units[0].text, "А"); // <041> pads to0410.
    let (mut duplicate, _) = unicode_book(
        &[SHOWN],
        "2 beginbfchar <001> <041> <0010> <0041> endbfchar",
    );
    assert_eq!(duplicate.refused(), "Duplicate PDF Unicode mapping");
}

#[test]
fn operation_eligibility_and_document_totals_precede_semantic_projection() {
    let (mut book, ids) = unicode_book(&[SHOWN], ONE);
    book.doc.objects.insert(
        ids.map,
        Stream::new(dictionary! {}, b"q ".repeat(4097)).into(),
    );
    assert_eq!(book.refused(), "PDF CMap operation bound");
    let mut book = Book::new(&[SHOWN]);
    clear_fonts(&mut book);
    let mut body = String::new();
    for n in 1..=1000 {
        body.push_str(&format!("1 beginbfchar <{n:04x}> <0041> endbfchar\n"));
    }
    for n in 0..9 {
        let map = add_map(&mut book, cmap(&body), true);
        add_font(&mut book, map, &format!("U{n}"));
    }
    assert_eq!(book.refused(), "PDF cumulative CMap operation bound");
}

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        if self.0.exists() {
            let root = self.0.canonicalize().unwrap();
            let temp = std::env::temp_dir().canonicalize().unwrap();
            assert_eq!(root.parent(), Some(temp.as_path()));
            assert!(root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("buddy-pdf-unicode-cache-"));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
#[test]
fn actual_cache_preserves_profile_change_and_changed_unicode_identity_without_a_schema_change() {
    let temp =
        Temp(std::env::temp_dir().join(format!("buddy-pdf-unicode-cache-{}", Uuid::new_v4())));
    let store = Store::open(StorePaths {
        data: temp.0.join("data"),
        cache: temp.0.join("cache"),
        credentials: temp.0.join("credentials"),
    })
    .unwrap();
    let cache = DiskCache::open(&store).unwrap();
    let mut simple = Book::new(&[ALPHA]);
    let current = normalize(&mut simple).document;
    let mut old = current.clone();
    old.extractor_revision = "pdf-simple-latin-r1-lopdf-0.46.0".into();
    let old_key = cache.install(old).unwrap();
    let key = cache.install(current).unwrap();
    assert_ne!(old_key, key);
    assert!(cache.load(&old_key).is_err());
    assert_eq!(
        found(&cache.load(&key).unwrap().unwrap(), "acceleration"),
        1
    );
    let (mut book, ids) = unicode_book(&[SHOWN], ONE);
    let first = normalize(&mut book).document;
    let first_key = cache.install(first.clone()).unwrap();
    assert_eq!(found(&cache.load(&first_key).unwrap().unwrap(), "α"), 1);
    book.doc.objects.insert(
        ids.map,
        Stream::new(
            dictionary! {},
            cmap("1 beginbfchar <0001> <03b2> endbfchar"),
        )
        .into(),
    );
    let second = normalize(&mut book).document;
    assert_ne!(second.source_sha256, first.source_sha256);
    let second_key = cache.install(second).unwrap();
    assert_ne!(second_key, first_key);
    assert!(cache.load(&first_key).is_err());
    assert_eq!(found(&cache.load(&second_key).unwrap().unwrap(), "β"), 1);
    book.doc.objects.insert(
        ids.map,
        Stream::new(
            dictionary! {},
            cmap("1 beginbfchar <0001> <d800> endbfchar"),
        )
        .into(),
    );
    assert!(pdf::normalize(Uuid::from_u128(1), book.bytes().as_slice()).is_err());
    assert_eq!(found(&cache.load(&second_key).unwrap().unwrap(), "β"), 1);
}
