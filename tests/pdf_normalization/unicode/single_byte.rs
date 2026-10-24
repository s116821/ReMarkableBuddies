use super::*;

fn one_map(body: &str) -> Vec<u8> {
    String::from_utf8(cmap(body))
        .unwrap()
        .replace("<0000> <ffff>", "<00> <ff>")
        .into_bytes()
}
fn simple(book: &mut Book, map: ObjectId, alias: &str, first: i64, last: i64) -> Ids {
    let ids = add_font(book, map, alias);
    let font = book.doc.get_dictionary_mut(ids.font).unwrap();
    font.set("Subtype", "TrueType");
    font.set("Encoding", "WinAnsiEncoding");
    font.remove(b"DescendantFonts");
    font.set("FontDescriptor", ids.descriptor);
    font.set("FirstChar", first);
    font.set("LastChar", last);
    font.set(
        "Widths",
        vec![Object::Integer(500); (last - first + 1) as usize],
    );
    ids
}

#[test]
fn explicit_single_byte_truetype_reaches_unicode_index() {
    let mut book = Book::new(&[b"BT /S0 12 Tf <0102030405> Tj ET"]);
    let map=add_map(&mut book,one_map("5 beginbfchar <01> <03b1> <02> <0020> <03> <00660069> <04> <0020> <05> <d83dde00> endbfchar"),true);
    simple(&mut book, map, "S0", 1, 5);
    let normalized = normalize(&mut book);
    assert_eq!(normalized.document.units[0].text, "α fi 😀");
    let index = Index::build(normalized.document).unwrap();
    assert_eq!(found(&index, "α"), 1);
    assert_eq!(found(&index, "fi"), 1);
}

fn local(book: &mut Book, page: usize, alias: &str, font: ObjectId) {
    let mut fonts = Dictionary::new();
    fonts.set(alias, font);
    book.doc
        .get_dictionary_mut(book.pages[page])
        .unwrap()
        .set("Resources", dictionary! {"Font"=>fonts});
}
fn one_book(content: &[&[u8]], body: &str, first: i64, last: i64) -> (Book, Ids) {
    let mut book = Book::new(content);
    let map = add_map(&mut book, one_map(body), false);
    let ids = simple(&mut book, map, "S0", first, last);
    (book, ids)
}
const SINGLE: &str = "1 beginbfchar <01> <03b1> endbfchar";

#[test]
fn scalar_array_ranges_and_all_256_single_byte_codes_are_bounded() {
    let (mut book,_)=one_book(&[b"BT /S0 12 Tf <0102030405> Tj ET"],
        "1 beginbfrange <01> <02> <0041> endbfrange 1 beginbfrange <03> <04> [<03b1> <03b2>] endbfrange 1 beginbfchar <05> <00660069> endbfchar",1,5);
    assert_eq!(normalize(&mut book).document.units[0].text, "ABαβfi");
    let mut body = String::new();
    for start in (0..256).step_by(100) {
        let end = (start + 100).min(256);
        body.push_str(&format!("{} beginbfchar ", end - start));
        for code in start..end {
            body.push_str(&format!("<{code:02x}> <0061> "));
        }
        body.push_str("endbfchar ");
    }
    let shown = format!(
        "BT /S0 12 Tf <{}> Tj ET",
        (0..256).map(|n| format!("{n:02x}")).collect::<String>()
    );
    let (mut book, _) = one_book(&[shown.as_bytes()], &body, 0, 255);
    assert_eq!(normalize(&mut book).document.units[0].text, "a".repeat(256));
}

#[test]
fn mixed_profiles_same_map_name_and_blank_pages_preserve_page_identity() {
    let mut book = Book::new(&[ALPHA, b"", b"BT /S0 12 Tf <01> Tj ET", SHOWN]);
    let map = add_map(&mut book, one_map(SINGLE), true);
    simple(&mut book, map, "S0", 1, 1);
    let map = add_map(&mut book, cmap(ONE), true);
    add_font(&mut book, map, "U0");
    let bytes = book.bytes();
    let result = pdf::normalize(Uuid::from_u128(1), bytes.as_slice()).unwrap();
    assert_eq!(result.document.source_sha256, digest(&bytes));
    assert_eq!(result.textless_pages, [2]);
    assert_eq!(result.document.units[2].text, "α");
    assert_eq!(result.document.units[3].text, "α");
    let mixed = Index::build(result.document).unwrap();
    let matches = mixed.query(mixed.key(), "α", 4, 8192, false).unwrap();
    assert_eq!(
        matches.passages.iter().map(|p| p.page).collect::<Vec<_>>(),
        [Some(3), Some(4)]
    );
    // The legacy declaration uses a Type0-only source admitted by the old profile.
    let (mut compatible, _) = unicode_book(&[SHOWN], ONE);
    let current = normalize(&mut compatible).document;
    let mut legacy = current.clone();
    legacy.extractor_revision = "pdf-explicit-unicode-r2-lopdf-0.46.0".into();
    let old = Index::build(legacy.clone()).unwrap().key().clone();
    let mut cache = remarkable_reader_buddy::retrieval::Cache::default();
    cache.install(legacy).unwrap();
    cache.install(current).unwrap();
    let index = cache.get(Uuid::from_u128(1)).unwrap();
    assert_ne!(index.key(), &old);
    assert!(index.query(&old, "α", 2, 4096, false).is_err());
    assert_eq!(found(index, "α"), 1);
}

#[test]
fn shared_reference_width_conflicts_refuse_in_both_controlled_orders() {
    for one_first in [true, false] {
        let mut book = Book::new(&[b"BT /A 12 Tf <0001> Tj ET", b"BT /B 12 Tf <0001> Tj ET"]);
        let bytes = if one_first {
            one_map("2 beginbfchar <00> <0041> <01> <0042> endbfchar")
        } else {
            cmap("2 beginbfchar <0000> <0041> <0001> <0042> endbfchar")
        };
        let map = add_map(&mut book, bytes, true);
        let one = simple(&mut book, map, "S", 0, 1);
        let two = add_font(&mut book, map, "U");
        local(
            &mut book,
            0,
            "A",
            if one_first { one.font } else { two.font },
        );
        local(
            &mut book,
            1,
            "B",
            if one_first { two.font } else { one.font },
        );
        assert!(book.refused().contains("cached map width mismatch"));
    }
}

#[test]
fn cached_map_still_checks_each_fonts_own_character_range() {
    let (mut book, first) = one_book(
        &[b"BT /A 12 Tf <01> Tj ET", b"BT /B 12 Tf <01> Tj ET"],
        "2 beginbfchar <01> <0041> <02> <0042> endbfchar",
        1,
        2,
    );
    let second = simple(&mut book, first.map, "S1", 1, 1);
    local(&mut book, 0, "A", first.font);
    local(&mut book, 1, "B", second.font);
    assert!(book.refused().contains("outside font character range"));
}

#[test]
fn malformed_font_metadata_and_wrong_source_widths_have_no_fallback() {
    for damage in 0..17 {
        let (mut book, ids) = one_book(&[b"BT /S0 12 Tf <01> Tj ET"], SINGLE, 1, 1);
        match damage {
            0 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Encoding", "MacRomanEncoding"),
            1 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Encoding", dictionary! {}),
            2 => {
                book.doc
                    .get_dictionary_mut(ids.font)
                    .unwrap()
                    .remove(b"FirstChar");
            }
            3 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("FirstChar", -1),
            4 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("LastChar", 256),
            5 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("FirstChar", 2),
            6 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Widths", Vec::<Object>::new()),
            7 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Widths", vec![Object::Name(b"invalid".to_vec())]),
            8 => book
                .doc
                .get_dictionary_mut(ids.descriptor)
                .unwrap()
                .set("FontName", "Foreign"),
            9 => {
                book.doc
                    .get_dictionary_mut(ids.font)
                    .unwrap()
                    .remove(b"FontDescriptor");
            }
            10 => book
                .doc
                .get_dictionary_mut(ids.descriptor)
                .unwrap()
                .set("CIDSet", Object::Null),
            11 => {
                let program = book.doc.add_object(Stream::new(
                    dictionary! {"F"=>Object::string_literal("/external")},
                    vec![],
                ));
                book.doc
                    .get_dictionary_mut(ids.descriptor)
                    .unwrap()
                    .set("FontFile2", program);
            }
            12 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Unknown", 1),
            13 => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Subtype", "Type3"),
            14 => {
                let map = add_map(
                    &mut book,
                    one_map("1 beginbfchar <0001> <03b1> endbfchar"),
                    false,
                );
                book.doc
                    .get_dictionary_mut(ids.font)
                    .unwrap()
                    .set("ToUnicode", map);
            }
            15 => {
                let map = add_map(
                    &mut book,
                    one_map("1 beginbfchar <01> <d800> endbfchar"),
                    false,
                );
                book.doc
                    .get_dictionary_mut(ids.font)
                    .unwrap()
                    .set("ToUnicode", map);
            }
            _ => book
                .doc
                .get_dictionary_mut(ids.font)
                .unwrap()
                .set("Widths", vec![Object::Integer(500); 65537]),
        }
        let error = book.refused();
        assert!(!error.is_empty(), "case {damage}");
        if damage == 16 {
            assert!(error.contains("metadata work bound"));
        }
    }
    let (mut book, ids) = one_book(&[ALPHA, b"BT /S0 12 Tf <41> Tj ET"], SINGLE, 1, 65);
    let base = book.font;
    local(&mut book, 0, "F1", base);
    local(&mut book, 1, "S0", ids.font);
    assert!(book.refused().contains("Unmapped PDF Unicode code"));
}

#[test]
fn indirect_metadata_repeated_fonts_and_aggregate_font_bound_are_preserved() {
    let shown = b"BT /S0 12 Tf <01> Tj ET";
    let (mut book, ids) = one_book(&[shown.as_slice(); 32], SINGLE, 1, 1);
    let first = book.doc.add_object(Object::Integer(1));
    let widths = book.doc.add_object(Object::Array(vec![500.into()]));
    let encoding = book
        .doc
        .add_object(Object::Name(b"WinAnsiEncoding".to_vec()));
    let font = book.doc.get_dictionary_mut(ids.font).unwrap();
    font.set("FirstChar", first);
    font.set("LastChar", first);
    font.set("Widths", widths);
    font.set("Encoding", encoding);
    assert_eq!(normalize(&mut book).document.units.len(), 32);
    let mut book = Book::new(&[shown, shown]);
    clear_fonts(&mut book);
    let map = add_map(&mut book, one_map(SINGLE), true);
    for i in 0..64 {
        simple(&mut book, map, &format!("S{i}"), 1, 1);
    }
    assert_eq!(normalize(&mut book).document.units[0].text, "α");
    let first_fonts = font_table(&mut book).clone();
    let overflow = simple(&mut book, map, "overflow", 1, 1);
    book.doc
        .get_dictionary_mut(book.pages[0])
        .unwrap()
        .set("Resources", dictionary! {"Font"=>first_fonts});
    local(&mut book, 1, "S0", overflow.font);
    assert!(book.refused().contains("font count bound"));
}
