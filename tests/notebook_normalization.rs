//! Authored synthetic controls; these do not establish handwriting accuracy.
use anyhow::{bail, Result};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use remarkable_reader_buddy::{
    llm::LLMEngine,
    retrieval::{disk::DiskCache, notebook::*, Index, Provider},
    storage::{Store, StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    io::Cursor,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
};

fn png() -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(3, 4, Rgb([20, 30, 40])))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

// Independent slow CRC for malformed-container controls, not production parsing.
fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = !0u32;
    for b in &out[4..] {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xedb88320
            };
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
    out
}

#[test]
fn animated_huge_header_and_crc_valid_broken_deflate_refuse_before_dispatch() {
    let original = png();
    let mut animated = original[..33].to_vec();
    animated.extend(chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]));
    animated.extend_from_slice(&original[33..]);
    let mut huge_header = original[16..29].to_vec();
    huge_header[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    let mut huge = original[..8].to_vec();
    huge.extend(chunk(b"IHDR", &huge_header));
    huge.extend_from_slice(&original[33..]);
    let mut broken = original[..33].to_vec();
    broken.extend(chunk(b"IDAT", b"not deflate but correct CRC"));
    broken.extend(chunk(b"IEND", b""));
    for bytes in [animated, huge, broken] {
        let mut e = engine(response(9, "text"));
        assert!(run(&mut e, &bytes, Options::default()).is_err());
        clean(&e, 0);
    }
}

#[test]
fn whole_set_bounds_and_partial_text_limits() {
    let b = png();
    let pages = [
        PageImage { page: 9, png: &b },
        PageImage { page: 2, png: &b },
    ];
    for options in [
        Options {
            max_pages: 1,
            ..Default::default()
        },
        Options {
            max_page_bytes: b.len(),
            max_image_bytes: b.len(),
            ..Default::default()
        },
        Options {
            max_page_pixels: 12,
            max_total_pixels: 12,
            ..Default::default()
        },
    ] {
        let mut e = engine(response(9, "text"));
        assert!(normalize(
            Uuid::new_v4(),
            &pages,
            "m",
            &mut e,
            options,
            &mut |_| Ok(())
        )
        .is_err());
        clean(&e, 0);
    }
    let mut e = engine(
        json!({"pages":[{"page":9,"status":"transcribed","text":"123456"},{"page":2,"status":"transcribed","text":"123456"}]}),
    );
    assert!(normalize(
        Uuid::new_v4(),
        &pages,
        "m",
        &mut e,
        Options {
            max_page_text_bytes: 8,
            max_text_bytes: 8,
            ..Default::default()
        },
        &mut |_| Ok(())
    )
    .is_err());
    clean(&e, 1);
    let mut e = engine(response(9, "text"));
    assert!(catch_unwind(AssertUnwindSafe(|| normalize(
        Uuid::new_v4(),
        &pages,
        "m",
        &mut e,
        Options::default(),
        &mut |_| panic!("authored progress panic")
    )))
    .is_err());
    clean(&e, 0);
}
#[derive(Default)]
struct Engine {
    content: Vec<Value>,
    sent: Vec<Value>,
    reply: String,
    calls: usize,
    clears: usize,
    fail: bool,
    panic: bool,
    swallow: bool,
}
impl LLMEngine for Engine {
    fn add_text_content(&mut self, s: &str) {
        self.content.push(json!({"type":"text","text":s}));
    }
    fn add_image_content(&mut self, s: &str) {
        self.content.push(json!({"type":"image_url","image_url":s}));
    }
    fn clear_content(&mut self) {
        self.clears += 1;
        self.content.clear();
    }
    fn execute(&mut self) -> Result<String> {
        self.execute_with_progress(&mut || Ok(()))
    }
    fn execute_with_progress(&mut self, p: &mut dyn FnMut() -> Result<()>) -> Result<String> {
        self.calls += 1;
        self.sent = self.content.clone();
        let first = p();
        if !self.swallow {
            first?;
        } else {
            let _ = p();
        }
        assert!(!self.panic, "authored engine panic");
        if self.fail {
            bail!("sensitive provider body MUST_NOT_ESCAPE");
        }
        Ok(self.reply.clone())
    }
}
fn engine(reply: Value) -> Engine {
    Engine {
        reply: reply.to_string(),
        content: vec![json!("old secret text/image")],
        ..Default::default()
    }
}
fn response(page: u32, text: &str) -> Value {
    json!({"pages":[{"page":page,"status":"transcribed","text":text}]})
}
fn run(e: &mut Engine, bytes: &[u8], opts: Options) -> Result<NotebookText> {
    normalize(
        Uuid::from_u128(7),
        &[PageImage {
            page: 9,
            png: bytes,
        }],
        "declared-model-v1",
        e,
        opts,
        &mut |_| Ok(()),
    )
}
fn clean(e: &Engine, calls: usize) {
    assert_eq!(e.calls, calls);
    assert_eq!(e.clears, 2);
    assert!(e.content.is_empty());
}

#[test]
fn actual_ordered_png_to_index_cache_reopen_and_query_retains_unverified_key() {
    let bytes = png();
    let mut e = engine(
        json!({"pages":[{"page":9,"status":"transcribed","text":"Mattie Walden studies at Hampton. café"},{"page":2,"status":"transcribed","text":"Second year in Junior class."}]}),
    );
    let doc = normalize(
        Uuid::from_u128(7),
        &[
            PageImage {
                page: 9,
                png: &bytes,
            },
            PageImage {
                page: 2,
                png: &bytes,
            },
        ],
        "declared-model-v1",
        &mut e,
        Options::default(),
        &mut |_| Ok(()),
    )
    .unwrap();
    clean(&e, 1);
    assert_eq!(
        doc.document
            .units
            .iter()
            .map(|u| u.page)
            .collect::<Vec<_>>(),
        vec![Some(9), Some(2)]
    );
    assert_eq!(doc.authority, "unverified-historical-page-image-ocr");
    assert!(doc
        .document
        .extractor_revision
        .starts_with("notebook-ocr-unverified-v1:"));
    assert_eq!(
        doc.prompt_sha256,
        remarkable_reader_buddy::storage::digest(&serde_json::to_vec(&e.sent).unwrap())
    );
    assert_eq!(doc.prompt_bytes, serde_json::to_vec(&e.sent).unwrap().len());
    assert_eq!(e.sent.len(), 6);
    assert!(e.sent.iter().all(|p| !p.to_string().contains("old secret")));
    let index = Index::build(doc.document.clone()).unwrap();
    let key = index.key().clone();
    let root = std::env::temp_dir().join(format!("notebook-test-{}", Uuid::new_v4()));
    let _cleanup = Cleanup(root.clone());
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    {
        let store = Store::open(paths.clone()).unwrap();
        let cache = DiskCache::open(&store).unwrap();
        assert_eq!(cache.install(doc.document.clone()).unwrap(), key);
    }
    {
        let store = Store::open(paths).unwrap();
        let cache = DiskCache::open(&store).unwrap();
        let found = cache.query(&key, "Junior class", 2, 8192, false).unwrap();
        assert_eq!(found.source, key);
        assert_eq!(found.passages[0].page, Some(2));
        let passage = &found.passages[0];
        assert_eq!(
            passage.text,
            doc.document.units[passage.unit].text[passage.range[0]..passage.range[1]]
        );
        let mut stale = key.clone();
        stale.extractor_revision.push('x');
        assert!(cache.query(&stale, "Mattie", 2, 8192, false).is_err());
        let mut invalid = doc.document.clone();
        invalid.units[0].page = Some(0);
        assert!(cache.install(invalid).is_err());
        assert!(!cache
            .query(&key, "Mattie", 2, 8192, false)
            .unwrap()
            .passages
            .is_empty());
    }
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn identity_changes_with_bytes_order_mapping_profile_and_normalized_text() {
    let b = png(); // A second valid image, not an invalid byte mutation.
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(3, 4, Rgb([21, 30, 40])))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    let other = out.into_inner();
    let get = |pages: &[PageImage<'_>], model: &str, text: &str| {
        let mut e = engine(
            json!({"pages":pages.iter().map(|p|json!({"page":p.page,"status":"transcribed","text":text})).collect::<Vec<_>>() }),
        );
        Index::build(
            normalize(
                Uuid::from_u128(7),
                pages,
                model,
                &mut e,
                Options::default(),
                &mut |_| Ok(()),
            )
            .unwrap()
            .document,
        )
        .unwrap()
        .key()
        .clone()
    };
    let first = get(
        &[
            PageImage { page: 9, png: &b },
            PageImage {
                page: 2,
                png: &other,
            },
        ],
        "v1",
        "text",
    );
    let reorder = get(
        &[
            PageImage {
                page: 2,
                png: &other,
            },
            PageImage { page: 9, png: &b },
        ],
        "v1",
        "text",
    );
    assert_ne!(first.source_sha256, reorder.source_sha256);
    let content = get(
        &[
            PageImage {
                page: 9,
                png: &other,
            },
            PageImage {
                page: 2,
                png: &other,
            },
        ],
        "v1",
        "text",
    );
    assert_ne!(first.source_sha256, content.source_sha256);
    let mapping = get(
        &[
            PageImage { page: 8, png: &b },
            PageImage {
                page: 2,
                png: &other,
            },
        ],
        "v1",
        "text",
    );
    assert_ne!(first.source_sha256, mapping.source_sha256);
    let profile = get(
        &[
            PageImage { page: 9, png: &b },
            PageImage {
                page: 2,
                png: &other,
            },
        ],
        "v2",
        "text",
    );
    assert_eq!(first.source_sha256, profile.source_sha256);
    assert_ne!(first.extractor_revision, profile.extractor_revision);
    let normalized = get(
        &[
            PageImage { page: 9, png: &b },
            PageImage {
                page: 2,
                png: &other,
            },
        ],
        "v1",
        "changed",
    );
    assert_eq!(first.source_sha256, normalized.source_sha256);
    assert_ne!(first.normalized_sha256, normalized.normalized_sha256);
}

#[test]
fn invalid_identity_mapping_options_and_images_refuse_before_dispatch_and_clear() {
    let b = png();
    for (id, model, pages) in [
        (Uuid::nil(), "m", vec![PageImage { page: 1, png: &b }]),
        (Uuid::new_v4(), "", vec![PageImage { page: 1, png: &b }]),
        (Uuid::new_v4(), "m", vec![]),
        (Uuid::new_v4(), "m", vec![PageImage { page: 0, png: &b }]),
        (
            Uuid::new_v4(),
            "m",
            vec![
                PageImage { page: 1, png: &b },
                PageImage { page: 1, png: &b },
            ],
        ),
    ] {
        let mut e = engine(response(1, "text"));
        assert!(
            normalize(id, &pages, model, &mut e, Options::default(), &mut |_| Ok(
                ()
            ))
            .is_err()
        );
        clean(&e, 0);
    }
    let mut corrupt = b.clone();
    corrupt[40] ^= 1;
    let mut trailing = b.clone();
    trailing.push(0);
    for bad in [
        vec![],
        b"\xff\xd8not a PNG".to_vec(),
        corrupt,
        trailing,
        b[..b.len() - 1].to_vec(),
    ] {
        let mut e = engine(response(9, "text"));
        assert!(run(&mut e, &bad, Options::default()).is_err());
        clean(&e, 0);
    }
    let opts = [
        Options {
            max_pages: 0,
            ..Default::default()
        },
        Options {
            max_pages: 5,
            ..Default::default()
        },
        Options {
            max_image_bytes: 1,
            ..Default::default()
        },
        Options {
            max_total_pixels: 1,
            ..Default::default()
        },
        Options {
            max_response_bytes: 0,
            ..Default::default()
        },
        Options {
            max_side: 4097,
            ..Default::default()
        },
    ];
    for o in opts {
        let mut e = engine(response(9, "text"));
        assert!(run(&mut e, &b, o).is_err());
        clean(&e, 0);
    }
}

#[test]
fn real_decode_and_complete_input_budgets_refuse_without_engine_call() {
    let b = png();
    for o in [
        Options {
            max_page_bytes: 1,
            ..Default::default()
        },
        Options {
            max_side: 2,
            ..Default::default()
        },
        Options {
            max_page_pixels: 11,
            ..Default::default()
        },
        Options {
            max_input_bytes: 1,
            ..Default::default()
        },
    ] {
        let mut e = engine(response(9, "text"));
        assert!(run(&mut e, &b, o).is_err());
        clean(&e, 0);
    }
}

#[test]
fn malformed_incomplete_reordered_unreadable_and_duplicate_json_refuse() {
    let b = png();
    let bad = vec![
        "not JSON".into(),
        "```json\n{}\n```".into(),
        "{\"pages\":[],\"pages\":[]}".into(),
        json!({"pages":[],"extra":true}).to_string(),
        json!({"pages":[]}).to_string(),
        response(8, "wrong mapping").to_string(),
        json!({"pages":[{"page":9,"status":"unreadable","text":""}]}).to_string(),
        json!({"pages":[{"page":9,"status":"blank","text":"invented"}]}).to_string(),
        response(9, " \n").to_string(),
        response(9, "bad\u{0001}control").to_string(),
        json!({"pages":[{"page":9,"status":"transcribed","text":"text","extra":1}]}).to_string(),
    ];
    for reply in bad {
        let mut e = Engine {
            reply,
            ..engine(response(9, "text"))
        };
        assert!(run(&mut e, &b, Options::default()).is_err());
        clean(&e, 1);
    }
    for pages in [
        json!([{"page":2,"status":"transcribed","text":"two"},{"page":9,"status":"transcribed","text":"nine"}]),
        json!([{"page":9,"status":"transcribed","text":"nine"},{"page":9,"status":"transcribed","text":"duplicate"}]),
        json!([{"page":9,"status":"transcribed","text":"nine"},{"page":2,"status":"unreadable","text":""}]),
    ] {
        let mut e = engine(json!({"pages":pages}));
        assert!(normalize(
            Uuid::new_v4(),
            &[
                PageImage { page: 9, png: &b },
                PageImage { page: 2, png: &b }
            ],
            "m",
            &mut e,
            Options::default(),
            &mut |_| Ok(())
        )
        .is_err());
        clean(&e, 1);
    }
}

#[test]
fn explicit_blank_preserved_and_complete_response_text_result_bounds_enforced() {
    let b = png();
    let mut e = engine(json!({"pages":[{"page":9,"status":"blank","text":""}]}));
    let d = run(&mut e, &b, Options::default()).unwrap();
    assert_eq!(d.blank_pages, vec![9]);
    assert_eq!(d.document.units[0].text, "");
    clean(&e, 1);
    for o in [
        Options {
            max_response_bytes: 1,
            ..Default::default()
        },
        Options {
            max_page_text_bytes: 1,
            ..Default::default()
        },
        Options {
            max_page_text_bytes: 2,
            max_text_bytes: 2,
            ..Default::default()
        },
        Options {
            max_result_bytes: 1,
            ..Default::default()
        },
    ] {
        let mut e = engine(response(9, "long text"));
        assert!(run(&mut e, &b, o).is_err());
        clean(&e, 1);
    }
}

#[test]
fn all_progress_cancellations_latched_even_when_engine_swallows_and_late_result_refused() {
    let b = png();
    for target in [
        Progress::BeforeDecode(0),
        Progress::AfterDecode(0),
        Progress::BeforeDispatch,
        Progress::EngineProgress,
        Progress::AfterResponse,
        Progress::BeforeReturn,
    ] {
        let mut e = Engine {
            swallow: true,
            ..engine(response(9, "text"))
        };
        let mut hits = 0;
        let result = normalize(
            Uuid::new_v4(),
            &[PageImage { page: 9, png: &b }],
            "m",
            &mut e,
            Options::default(),
            &mut |stage| {
                if stage == target {
                    hits += 1;
                    bail!("private cancel reason");
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(hits, 1);
        clean(
            &e,
            usize::from(matches!(
                target,
                Progress::EngineProgress | Progress::AfterResponse | Progress::BeforeReturn
            )),
        );
    }
}
#[test]
fn provider_error_sanitized_and_unwinding_clears_temporary_engine_content() {
    let b = png();
    let mut e = Engine {
        fail: true,
        ..engine(response(9, "text"))
    };
    let error = run(&mut e, &b, Options::default()).err().unwrap();
    assert!(!format!("{error:#}").contains("MUST_NOT_ESCAPE"));
    clean(&e, 1);
    let mut e = Engine {
        panic: true,
        ..engine(response(9, "text"))
    };
    assert!(catch_unwind(AssertUnwindSafe(|| run(&mut e, &b, Options::default()))).is_err());
    clean(&e, 1);
}
