use super::*;
use serde_json::json;

const DOC: &str = "11111111-1111-4111-8111-111111111111";
const PAGES: [&str; 3] = [
    "22222222-2222-4222-8222-222222222222",
    "33333333-3333-4333-8333-333333333333",
    "44444444-4444-4444-8444-444444444444",
];
fn fixture() -> (CapturePlan, OpeningMetadata, Vec<SourceEntry>) {
    let plan = CapturePlan {
        visual_setup: None,
        document: DOC.into(),
        page: PAGES[1].into(),
        order: PAGES.iter().map(|p| (*p).into()).collect(),
        opening_metadata: None,
    };
    let policy = OpeningMetadata {
        content: json!({"fileType":"pdf", "pageCount":3, "sizeInBytes":"42",
            "cPages":{"pages":PAGES.iter().map(|id| json!({"id":id,"modifed":"17"})).collect::<Vec<_>>(),
            "lastOpened":{"timestamp":"1:15", "value":PAGES[0]},
            "unknown":[true,7,1.0,"x",null]}}).to_string(),
        metadata: json!({"lastOpened":"1000", "lastOpenedPage":0,"lastModified":"999",
            "unknown":{"keep":true,"number":1.0}}).to_string(),
    };
    let mut manifest: Vec<_> = [FileKind::Content, FileKind::Metadata]
        .into_iter()
        .map(|kind| SourceEntry {
            path: kind.path(&plan),
            sha256: Some(sha(policy.original(kind))),
        })
        .collect();
    for path in [
        DOC.to_string(),
        format!("{DOC}.pdf"),
        format!("{DOC}.local"),
        format!("{DOC}/{}.rm", PAGES[0]),
    ] {
        manifest.push(SourceEntry {
            sha256: if path == DOC {
                None
            } else {
                Some("99".repeat(32))
            },
            path,
        });
    }
    manifest.sort_by(|a, b| a.path.cmp(&b.path));
    (plan, policy, manifest)
}
fn changed(policy: &OpeningMetadata, kind: FileKind, path: &str, value: Value) -> Vec<u8> {
    let mut original = parse(policy.original(kind)).unwrap();
    *original.pointer_mut(path).unwrap() = value;
    serde_json::to_vec(&original).unwrap()
}

#[test]
fn original_bytes_bind_policy_to_manifest_and_full_fixture_order() {
    let (mut plan, mut policy, mut manifest) = fixture();
    policy.validate(&plan, &manifest).unwrap();
    policy.metadata.push(' ');
    assert!(policy.validate(&plan, &manifest).is_err());
    policy.metadata.pop();
    plan.order.swap(0, 2);
    assert!(policy.validate(&plan, &manifest).is_err());
    plan.order.swap(0, 2);
    manifest.push(SourceEntry {
        path: FileKind::Metadata.path(&plan),
        sha256: Some(sha(policy.metadata.as_bytes())),
    });
    assert!(policy.validate(&plan, &manifest).is_err());
}

#[test]
fn only_declared_activity_fields_may_advance_from_original() {
    let (plan, policy, _) = fixture();
    for kind in [FileKind::Content, FileKind::Metadata] {
        policy.verify(&plan, kind, policy.original(kind)).unwrap();
    }
    let mut metadata = parse(policy.metadata.as_bytes()).unwrap();
    metadata["lastOpened"] = json!("1001");
    metadata["lastOpenedPage"] = json!(1);
    policy
        .verify(
            &plan,
            FileKind::Metadata,
            &serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();
    let mut content = parse(policy.content.as_bytes()).unwrap();
    content["cPages"]["lastOpened"]["value"] = json!(PAGES[1]);
    content["cPages"]["lastOpened"]["timestamp"] = json!("1:16");
    policy
        .verify(
            &plan,
            FileKind::Content,
            &serde_json::to_vec(&content).unwrap(),
        )
        .unwrap();
    // Original remembered values remain permissible; no current-page claim.
    content["cPages"]["lastOpened"]["value"] = json!(PAGES[0]);
    policy
        .verify(
            &plan,
            FileKind::Content,
            &serde_json::to_vec(&content).unwrap(),
        )
        .unwrap();
}

#[test]
fn malformed_regressing_unrelated_or_unclocked_activity_refuses() {
    let (plan, policy, _) = fixture();
    for value in [
        json!(1001),
        json!("999"),
        json!("01000"),
        json!("+1000"),
        json!(""),
        json!("18446744073709551616"),
        json!(null),
    ] {
        assert!(policy
            .verify(
                &plan,
                FileKind::Metadata,
                &changed(&policy, FileKind::Metadata, "/lastOpened", value)
            )
            .is_err());
    }
    for value in [
        json!(1),
        json!(2),
        json!(3),
        json!(-1),
        json!(0.0),
        json!("0"),
        json!(null),
    ] {
        assert!(policy
            .verify(
                &plan,
                FileKind::Metadata,
                &changed(&policy, FileKind::Metadata, "/lastOpenedPage", value)
            )
            .is_err());
    }
    for value in [
        json!("2:16"),
        json!("1:14"),
        json!("01:15"),
        json!("1:015"),
        json!("1:15:2"),
        json!("1:18446744073709551616"),
        json!(null),
    ] {
        assert!(policy
            .verify(
                &plan,
                FileKind::Content,
                &changed(
                    &policy,
                    FileKind::Content,
                    "/cPages/lastOpened/timestamp",
                    value
                )
            )
            .is_err());
    }
    for value in [json!(PAGES[1]), json!(PAGES[2]), json!(DOC), json!(null)] {
        assert!(policy
            .verify(
                &plan,
                FileKind::Content,
                &changed(
                    &policy,
                    FileKind::Content,
                    "/cPages/lastOpened/value",
                    value
                )
            )
            .is_err());
    }
}

#[test]
fn unknown_fields_content_order_and_types_stay_protected() {
    let (plan, policy, _) = fixture();
    for (kind, path, value) in [
        (FileKind::Metadata, "/lastModified", json!("1001")),
        (FileKind::Metadata, "/unknown/keep", json!(false)),
        (FileKind::Metadata, "/unknown/number", json!(1)),
        (FileKind::Content, "/sizeInBytes", json!("43")),
        (FileKind::Content, "/pageCount", json!(4)),
        (FileKind::Content, "/cPages/pages/0/modifed", json!("18")),
        (FileKind::Content, "/cPages/pages/0/id", json!(PAGES[1])),
        (FileKind::Content, "/cPages/unknown/1", json!(7.0)),
    ] {
        assert!(
            policy
                .verify(&plan, kind, &changed(&policy, kind, path, value))
                .is_err(),
            "{path}"
        );
    }
    for kind in [FileKind::Metadata, FileKind::Content] {
        let mut value = parse(policy.original(kind)).unwrap();
        value["new"] = Value::Null;
        assert!(policy
            .verify(&plan, kind, &serde_json::to_vec(&value).unwrap())
            .is_err());
        let mut value = parse(policy.original(kind)).unwrap();
        value.as_object_mut().unwrap().remove("lastModified");
        if matches!(kind, FileKind::Metadata) {
            assert!(policy
                .verify(&plan, kind, &serde_json::to_vec(&value).unwrap())
                .is_err());
        }
    }
}

#[test]
fn recursive_duplicates_key_removal_and_oversized_samples_refuse() {
    let (plan, policy, _) = fixture();
    for bad in [
        policy
            .metadata
            .replacen("{", "{\"lastOpened\":\"1000\",", 1),
        policy
            .metadata
            .replace("\"keep\":true", "\"keep\":false,\"keep\":true"),
        policy.metadata.replace("\"lastOpened\":\"1000\",", ""),
        " ".repeat(MAX_JSON + 1),
        "[]".into(),
    ] {
        assert!(policy
            .verify(&plan, FileKind::Metadata, bad.as_bytes())
            .is_err());
    }
    assert!(parse(br#"{"a":[{"x":null,"x":null}]}"#).is_err());
    assert!(parse(br#"{"x":0} trailing"#).is_err());
}

#[test]
fn full_inventory_never_exempts_pdf_ink_local_files_directories_or_names() {
    let (plan, _, original) = fixture();
    verify_inventory(&plan, &original, &original).unwrap();
    for index in 0..original.len() {
        let mut changed = original
            .iter()
            .map(|e| SourceEntry {
                path: e.path.clone(),
                sha256: e.sha256.clone(),
            })
            .collect::<Vec<_>>();
        changed[index].sha256 = Some("88".repeat(32));
        let activity = [
            FileKind::Content.path(&plan),
            FileKind::Metadata.path(&plan),
        ]
        .contains(&changed[index].path);
        assert_eq!(
            verify_inventory(&plan, &original, &changed).is_ok(),
            activity
        );
        changed[index].sha256 = None;
        if original[index].sha256.is_some() {
            assert!(verify_inventory(&plan, &original, &changed).is_err());
        }
        changed[index].path.push('x');
        assert!(verify_inventory(&plan, &original, &changed).is_err());
    }
    assert!(verify_inventory(&plan, &original, &original[1..]).is_err());
}

#[test]
fn protected_numeric_tokens_are_lossless_before_comparison() {
    for (before, after) in [
        ("18446744073709551616", "18446744073709551617"),
        ("1.00000000000000001", "1.00000000000000002"),
        ("1e-400", "0.0"),
        ("-0.0", "0.0"),
        ("1e0", "1.0"),
    ] {
        let (plan, mut policy, _) = fixture();
        policy.metadata = policy
            .metadata
            .replace("\"number\":1.0", &format!("\"number\":{before}"));
        let changed = policy.metadata.replace(
            &format!("\"number\":{before}"),
            &format!("\"number\":{after}"),
        );
        assert_ne!(policy.metadata, changed);
        policy
            .verify(&plan, FileKind::Metadata, policy.metadata.as_bytes())
            .unwrap();
        assert!(
            policy
                .verify(&plan, FileKind::Metadata, changed.as_bytes())
                .is_err(),
            "{before} -> {after}"
        );
    }
    let (plan, mut policy, _) = fixture();
    policy.metadata = policy
        .metadata
        .replace("\"number\":1.0", "\"number\":18446744073709551616");
    let current = r#"{"unknown":{"number":18446744073709551616,"keep":true},"lastOpenedPage":0,"lastOpened":"1001","lastModified":"999"}"#;
    policy
        .verify(&plan, FileKind::Metadata, current.as_bytes())
        .unwrap();
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn sampled_files_must_match_inventory_and_remain_pinned_with_rejected_bytes_retained() {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let root = PathBuf::from(format!("/root/opening-samples-{}", uuid::Uuid::new_v4()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let source = root.join("source");
    let evidence = root.join("evidence");
    for dir in [&source, &evidence] {
        fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
    }
    let (plan, policy, mut manifest) = fixture();
    for kind in [FileKind::Content, FileKind::Metadata] {
        write_new(&source, &kind.path(&plan), policy.original(kind)).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let first = Samples::read(&plan, &source, &evidence, "first", &manifest, deadline).unwrap();
    first.verify(&policy, &plan).unwrap();
    let metadata = FileKind::Metadata.path(&plan);
    let updated = changed(&policy, FileKind::Metadata, "/lastOpened", json!("1001"));
    fs::write(source.join(&metadata), &updated).unwrap();
    assert!(first.verify(&policy, &plan).is_err());
    assert!(Samples::read(&plan, &source, &evidence, "stale", &manifest, deadline).is_err());
    assert_eq!(
        fs::read(evidence.join("stale.opening-metadata")).unwrap(),
        updated
    );
    manifest
        .iter_mut()
        .find(|e| e.path == metadata)
        .unwrap()
        .sha256 = Some(sha(&updated));
    let fresh = Samples::read(&plan, &source, &evidence, "fresh", &manifest, deadline).unwrap();
    fresh.verify(&policy, &plan).unwrap();
    fs::rename(
        source.join(&metadata),
        source.join("retained-original.metadata"),
    )
    .unwrap();
    write_new(&source, &metadata, &updated).unwrap();
    assert!(fresh.verify(&policy, &plan).is_err());
    assert_eq!(
        fs::read(evidence.join("fresh.opening-metadata")).unwrap(),
        updated
    );
}
