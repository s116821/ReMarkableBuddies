//! Actual existing selectors on a ready retained notebook; no provider/tablet.
use buddy::storage::*;
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[path = "support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;

fn run(f: &Fixture, product: bool, operation: &str, input: Option<&Value>) -> Value {
    let family = if product { "product" } else { "research" };
    let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    command
        .env_clear()
        .current_dir(&f.root)
        .env("REMARKABLE_BUDDIES_CONFIG", f.root.join("config.json"))
        .env("OPENAI_API_KEY", "PRIVATE_UNUSED")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1/unused")
        .args([format!("--{family}-settings-operation"), operation.into()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.args([format!("--{family}-settings-input"), "-".into()]);
    }
    let mut child = command.spawn().unwrap();
    let pipe = child.stdin.take().unwrap();
    let writer = input.map(|input| {
        let bytes = serde_json::to_vec(input).unwrap();
        std::thread::spawn(move || {
            let mut pipe = pipe;
            pipe.write_all(&bytes).unwrap();
        })
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CLI deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(writer) = writer {
        writer.join().unwrap();
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    assert!(result.stdout.len() <= 16_384 && result.stdout.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("PRIVATE"));
    serde_json::from_slice(&result.stdout).unwrap()
}
fn exercise(product: bool, notebook: bool) {
    let f = Fixture::new();
    if notebook {
        let store = Store::open(f.paths()).unwrap();
        let record = Envelope {
            envelope_version: 1,
            namespace: Namespace::SubjectMemory,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: store.actor_id,
            kind: Kind::Value,
            payload: json!({"fixture":"retained"}),
            media_descriptors: vec![],
        };
        store.commit(vec![record], BTreeMap::new()).unwrap();
        drop(store);
        drop(Store::prepare_notebook(f.paths()).unwrap());
    } else {
        drop(f.selected(false).0);
    }
    let original = fs::read(f.root.join("config.json")).unwrap();
    let inventory = f.inventory();
    let current = run(&f, product, "inspect", None);
    let changes = if product {
        json!({"writer_enabled":{"action":"set","value":false}})
    } else {
        json!({"research_policy":{"action":"set","value":"disabled"}})
    };
    let mut request = json!({"operation":Uuid::new_v4(),"base_sha256":current["data"]["base_sha256"],
        "config_path_sha256":current["data"]["config_path_sha256"],"changes":changes});
    request[if product {
        "product_settings_version"
    } else {
        "research_settings_version"
    }] = json!(1);
    let prepared = run(&f, product, "prepare", Some(&request));
    assert_eq!(prepared["outcome"], "prepared");
    let input = if product {
        json!({"intent":prepared["data"]["intent"]})
    } else {
        json!({"intent":prepared["data"]["intent"],"changes":changes})
    };
    let owner = Store::open(f.paths()).unwrap();
    assert_eq!(run(&f, product, "apply", Some(&input))["status"], "busy");
    drop(owner);
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), original);
    let applied = run(&f, product, "apply", Some(&input));
    assert_eq!(applied["outcome"], "verified");
    assert_eq!(applied["disk_readback_verified"], true);
    assert_eq!(
        applied["runtime_activation"],
        if product {
            "restart-required"
        } else {
            "unverified"
        }
    );
    assert_eq!(
        run(
            &f,
            product,
            "status",
            Some(&json!({"intent":input["intent"]}))
        )["outcome"],
        "desired-file-observed"
    );
    assert_eq!(
        run(&f, product, "apply", Some(&input))["status"],
        "conflict"
    );
    assert_eq!(f.inventory(), inventory);
    let config = buddy::config::Config::load(Some(f.root.join("config.json"))).unwrap();
    if product {
        assert!(!config.admits_writer(false));
    } else {
        assert_eq!(
            config.research_policy_snapshot().unwrap().policy(),
            buddy::conversation::receiving::memory::research::ResearchPolicy::Disabled
        );
    }
    drop(Store::open(f.paths()).unwrap());
}
#[test]
fn actual_product_cli_notebook_roundtrip() {
    exercise(true, true);
}
#[test]
fn actual_research_cli_notebook_roundtrip() {
    exercise(false, true);
}

#[test]
fn actual_existing_selected_preferences_keep_legacy_admission() {
    exercise(true, false);
    exercise(false, false);
}
