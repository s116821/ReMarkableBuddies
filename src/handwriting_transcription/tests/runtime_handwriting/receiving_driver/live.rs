//! Explicitly admitted, single-process qualification fixture.
use super::*;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) const MODEL: &str = "gpt-6.1-sol";
const ENDPOINT: &str = "https://api.openai.com";
const LINCOLN_PAGES: [u32; 2] = [1, 2];
const BARTON_PAGES: [u32; 4] = [24, 35, 168, 170];

fn read(path: &Path, maximum: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= maximum, "input bound");
    Ok(bytes)
}

fn save(root: &Path, name: &str, value: &Value) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    anyhow::ensure!(bytes.len() <= 262144, "evidence bound");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(name))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn selection(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.strip_prefix("https://linear.app/magentumdragon/issue/REM-26#comment-")
            .is_some_and(|id| Uuid::parse_str(id).is_ok())
    })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    manifest_sha256: String,
    selection: Value,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    seed_gate_sha256: String,
    decision: String,
    selection: Value,
}

fn approve(raw: &[u8], expected: &str, decision: &str) -> anyhow::Result<Review> {
    let value: Review = serde_json::from_slice(raw)?;
    anyhow::ensure!(
        value.seed_gate_sha256 == expected
            && value.decision == decision
            && selection(&value.selection),
        "confirmation refused"
    );
    Ok(value)
}

struct Meter<'a, E> {
    inner: E,
    root: &'a Path,
    calls: usize,
    before_review: usize,
    after_review: usize,
    committed: &'a std::cell::Cell<bool>,
}

impl<E: LLMEngine> LLMEngine for Meter<'_, E> {
    fn add_text_content(&mut self, text: &str) {
        self.inner.add_text_content(text);
    }
    fn add_image_content(&mut self, image: &str) {
        self.inner.add_image_content(image);
    }
    fn clear_content(&mut self) {
        self.inner.clear_content();
    }
    fn execute(&mut self) -> anyhow::Result<String> {
        self.execute_with_progress(&mut || Ok(()))
    }
    fn execute_with_progress(
        &mut self,
        progress: &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<String> {
        anyhow::ensure!(
            self.calls
                < if self.committed.get() {
                    self.after_review
                } else {
                    self.before_review
                },
            "provider call budget"
        );
        self.calls += 1;
        save(
            self.root,
            &format!("call-{}-started.json", self.calls),
            &json!({"call":self.calls}),
        )?;
        let result = self.inner.execute_with_progress(progress);
        let response = match &result {
            Ok(text) => {
                let retained = text.len() <= 65536;
                if retained {
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(self.root.join(format!("call-{}-response.txt", self.calls)))?;
                    file.write_all(text.as_bytes())?;
                    file.sync_all()?;
                }
                json!({"bytes":text.len(),"sha256":digest(text.as_bytes()),"retained":retained})
            }
            Err(_) => Value::Null,
        };
        save(
            self.root,
            &format!("call-{}-ended.json", self.calls),
            &json!({"call":self.calls,"returned":result.is_ok(),"response":response}),
        )?;
        result
    }
}

fn inputs(root: &Path, pages: &[u32]) -> anyhow::Result<(Vec<Vec<u8>>, String)> {
    let raw_map = read(&root.join("png-map.json"), 8192)?;
    let map: Value = serde_json::from_slice(&raw_map)?;
    let map = map
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid image map"))?;
    anyhow::ensure!(map.len() == pages.len(), "image count");
    let mut images = Vec::new();
    for (page, fact) in pages.iter().copied().zip(map) {
        let png = read(&root.join(format!("{page}.png")), 8 * 1024 * 1024)?;
        anyhow::ensure!(
            fact["page"] == page
                && fact["png_sha256"] == digest(&png)
                && fact["png_bytes"] == png.len(),
            "image pin"
        );
        crate::retrieval::notebook::admit_target(7, png.clone(), &mut |_| Ok(()))?;
        images.push(png);
    }
    crate::handwriting::png_dimensions(&images[0])
        .map_err(|_| anyhow::anyhow!("seed correction PNG refused"))?;
    Ok((images, digest(&raw_map)))
}

fn run(root: &Path, pages: &[u32], seed_mode: SeedMode) -> anyhow::Result<()> {
    let manifest = read(&root.join("runtime.json"), 16384)?;
    let pin: Value = serde_json::from_slice(&manifest)?;
    let authority: Admission =
        serde_json::from_slice(&read(&root.join("live-admission.json"), 4096)?)?;
    let pinned_seed_mode = match seed_mode {
        SeedMode::CompletePage => None,
        SeedMode::Abstention => Some("explicit-abstention-enrollment"),
        SeedMode::EitherValidOutcome => Some("two-valid-seed-outcomes"),
    };
    anyhow::ensure!(
        authority.manifest_sha256 == digest(&manifest)
            && selection(&authority.selection)
            && pin["seed_mode"].as_str() == pinned_seed_mode
            && pin["model"] == MODEL
            && pin["endpoint"] == ENDPOINT
            && pin["executable_sha256"] == digest(&fs::read(std::env::current_exe()?)?),
        "live admission"
    );
    let (images, map_sha) = inputs(root, pages)?;
    let (seed, targets) = images.split_first().expect("fixture has a seed");
    anyhow::ensure!(
        pin["png_map_sha256"] == map_sha
            && !root.join("review.json").exists()
            && !root.join("store").exists(),
        "live input custody"
    );
    save(
        root,
        "consumed.json",
        &json!({"admission":authority.selection,"manifest_sha256":authority.manifest_sha256}),
    )?;
    let mut config = Config {
        paths: StorePaths {
            data: root.join("store"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        },
        ..Config::default()
    };
    config.handwriting = handwriting::Configuration {
        enabled: true,
        profile: Some(profile()),
    };
    let store = Store::open(config.paths.clone())?;
    store.set_sync_mode(
        &store.sync_mode_snapshot()?.token,
        Uuid::new_v4(),
        crate::storage::sync::mode::SyncModeChoice::LocalOnly,
    )?;
    drop(store);
    drop(Store::prepare_notebook(config.paths.clone())?);
    let key =
        std::env::var("OPENAI_API_KEY").map_err(|_| anyhow::anyhow!("credential unavailable"))?;
    anyhow::ensure!(!key.trim().is_empty(), "credential unavailable");
    let committed = std::cell::Cell::new(false);
    let mut engine = Meter {
        inner: crate::OpenAI::new(MODEL.into(), key, Some(ENDPOINT.into())).with_single_request(),
        root,
        calls: 0,
        before_review: 2 * images.len(),
        after_review: 2 * (images.len() + targets.len()),
        committed: &committed,
    };
    let mut cell = 0;
    let result = compare_gated(
        &config,
        seed,
        targets,
        &mut engine,
        |gate| {
            save(root, "seed-gate.json", &gate)?;
            let gate_sha = digest(&read(&root.join("seed-gate.json"), 262144)?);
            let until = Instant::now() + Duration::from_secs(600);
            while Instant::now() < until {
                if root.join("review.json").exists() {
                    let decision = gate_decision(&gate);
                    let review =
                        approve(&read(&root.join("review.json"), 4096)?, &gate_sha, decision)?;
                    let text = read(&root.join("seed-supported.txt"), 4096)?;
                    anyhow::ensure!(pin["seed_sha256"] == digest(&text), "seed text pin");
                    save(
                        root,
                        "confirmation-claimed.json",
                        &json!({"seed_gate_sha256":gate_sha,"selection":review.selection}),
                    )?;
                    return Ok(String::from_utf8(text)?);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            anyhow::bail!("seed review expired")
        },
        |value| {
            if value.get("confirmation_prepared").is_some() {
                save(root, "confirmation-prepared.json", &value)
            } else if value.get("confirmation").is_some() {
                save(root, "confirmation-result.json", &value)?;
                committed.set(value["committed"] == true);
                Ok(())
            } else {
                cell += 1;
                save(root, &format!("cell-{cell}.json"), &value)
            }
        },
        seed_mode,
    );
    save(
        root,
        "terminal.json",
        &json!({"complete":result.is_ok(),"calls":engine.calls,"cells":cell}),
    )?;
    result.map(|_| ())
}

#[test]
#[ignore = "requires exact reviewed live admission"]
fn live_barton_receiving() -> anyhow::Result<()> {
    let root = PathBuf::from(
        std::env::var_os("RMB_BARTON_ROOT")
            .ok_or_else(|| anyhow::anyhow!("fixture root absent"))?,
    );
    anyhow::ensure!(
        root.is_absolute() && root.canonicalize()?.is_dir(),
        "fixture root"
    );
    run(&root, &BARTON_PAGES, SeedMode::CompletePage)
}

#[test]
#[ignore = "requires local frozen Barton inputs"]
fn check_barton_inputs() -> anyhow::Result<()> {
    let root = PathBuf::from(
        std::env::var_os("RMB_BARTON_ROOT")
            .ok_or_else(|| anyhow::anyhow!("fixture root absent"))?,
    );
    inputs(&root, &BARTON_PAGES)?;
    Ok(())
}

fn lincoln_root(variable: &str) -> anyhow::Result<PathBuf> {
    let root = PathBuf::from(
        std::env::var_os(variable).ok_or_else(|| anyhow::anyhow!("fixture root absent"))?,
    );
    anyhow::ensure!(
        root.is_absolute() && root.canonicalize()?.is_dir(),
        "fixture root"
    );
    Ok(root)
}

#[test]
#[ignore = "requires exact reviewed live admission"]
fn live_lincoln_receiving() -> anyhow::Result<()> {
    run(
        &lincoln_root("RMB_LINCOLN_ROOT")?,
        &LINCOLN_PAGES,
        SeedMode::CompletePage,
    )
}

#[test]
#[ignore = "requires local frozen Lincoln inputs"]
fn check_lincoln_inputs() -> anyhow::Result<()> {
    inputs(&lincoln_root("RMB_LINCOLN_ROOT")?, &LINCOLN_PAGES)?;
    Ok(())
}

#[test]
#[ignore = "requires fresh explicit enrollment admission"]
fn live_lincoln_abstention_enrollment() -> anyhow::Result<()> {
    run(
        &lincoln_root("RMB_LINCOLN_ENROLLMENT_ROOT")?,
        &LINCOLN_PAGES,
        SeedMode::Abstention,
    )
}

#[test]
#[ignore = "requires fresh two-outcome seed admission"]
fn live_lincoln_two_seed_outcomes() -> anyhow::Result<()> {
    run(
        &lincoln_root("RMB_LINCOLN_TWO_OUTCOME_ROOT")?,
        &LINCOLN_PAGES,
        SeedMode::EitherValidOutcome,
    )
}

fn gate_decision(gate: &Value) -> &'static str {
    if gate["seed_outcome"] == "valid-whole-page-abstention" {
        "enroll-supported-controlled-seed-after-abstention"
    } else {
        "confirm-supported-controlled-seed"
    }
}

#[test]
fn review_rejects_unknown_or_stale_gate() {
    let valid = json!({"decision":"confirm-supported-controlled-seed","seed_gate_sha256":"exact",
        "selection":"https://linear.app/magentumdragon/issue/REM-26#comment-11111111-1111-1111-1111-111111111111"});
    let bytes = serde_json::to_vec(&valid).unwrap();
    assert!(approve(&bytes, "exact", "confirm-supported-controlled-seed").is_ok());
    assert!(approve(&bytes, "stale", "confirm-supported-controlled-seed").is_err());
    assert!(approve(b"{}", "exact", "confirm-supported-controlled-seed").is_err());
}

#[test]
fn metered_responses_and_refusals_survive_actual_receiving() {
    for (reply, generate, expected, succeeds) in [
        (
            good_reply(),
            "# Title\n\nA note.".to_string(),
            "writer-receiving",
            true,
        ),
        (
            "{}".into(),
            String::new(),
            "normalization-invalid-response",
            false,
        ),
        (
            "x".repeat(65537),
            String::new(),
            "normalization-invalid-response",
            false,
        ),
        (
            r#"{"pages":[{"page":7,"status":"unreadable","text":""}]}"#.into(),
            String::new(),
            "normalization-unreadable",
            false,
        ),
        (good_reply(), String::new(), "writer-receiving", false),
    ] {
        let mut local = Local::ready_notebook(true);
        enable(&mut local);
        let store = Arc::new(Store::open(local.config.paths.clone()).unwrap());
        let ledger = Ledger::new(store.clone());
        let mut dispatch = Dispatch::local(&local.config, &ledger).unwrap();
        let committed = std::cell::Cell::new(false);
        let mut fake = engine(1);
        fake.replies = [reply.clone(), generate].into();
        let mut meter = Meter {
            inner: fake,
            root: &local.root,
            calls: 0,
            before_review: 4,
            after_review: 6,
            committed: &committed,
        };
        let mut evidence = None;
        let result = receive(
            &store,
            &mut dispatch,
            &sample(3),
            &mut meter,
            &mut |value| {
                evidence = Some(value);
                Ok(())
            },
            false,
        );
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(evidence.unwrap()["diagnostic"], expected);
        let ended: Value =
            serde_json::from_slice(&fs::read(local.root.join("call-1-ended.json")).unwrap())
                .unwrap();
        assert_eq!(ended["response"]["bytes"], reply.len());
        assert_eq!(ended["response"]["sha256"], digest(reply.as_bytes()));
        if reply.len() <= 65536 {
            assert_eq!(
                fs::read(local.root.join("call-1-response.txt")).unwrap(),
                reply.as_bytes()
            );
        } else {
            assert_eq!(ended["response"]["retained"], false);
            assert!(!local.root.join("call-1-response.txt").exists());
        }
        assert_eq!(
            meter.calls,
            if expected == "writer-receiving" { 2 } else { 1 }
        );
    }
}

#[test]
fn two_outcome_gate_requires_the_decision_for_the_actual_valid_seed() {
    for (seed_reply, seed_generate, outcome, decision, wrong_decision, seed_calls) in [
        (
            good_reply(),
            "# Seed\n\nSynthetic reading.",
            "valid-whole-page-reading",
            "confirm-supported-controlled-seed",
            "enroll-supported-controlled-seed-after-abstention",
            2,
        ),
        (
            r#"{"pages":[{"page":7,"status":"unreadable","text":""}]}"#.into(),
            "",
            "valid-whole-page-abstention",
            "enroll-supported-controlled-seed-after-abstention",
            "confirm-supported-controlled-seed",
            1,
        ),
    ] {
        for submitted in [decision, wrong_decision] {
            let mut local = Local::ready_notebook(true);
            enable(&mut local);
            let mut model = engine(3);
            model.replies[2] = seed_reply.clone();
            if seed_calls == 1 {
                model.replies.remove(3);
            } else {
                model.replies[3] = seed_generate.into();
            }
            let mut committed = false;
            let result = compare_gated(
                &local.config,
                &sample(2),
                &[sample(3)],
                &mut model,
                |gate| {
                    assert_eq!(gate["seed_outcome"], outcome);
                    assert_eq!(gate["intent_owner"], "controlled-fixture");
                    assert_eq!(gate["normalized"].is_null(), seed_calls == 1);
                    let review = json!({"decision":submitted,"seed_gate_sha256":"exact",
                        "selection":"https://linear.app/magentumdragon/issue/REM-26#comment-11111111-1111-1111-1111-111111111111"});
                    approve(&serde_json::to_vec(&review)?, "exact", gate_decision(&gate))?;
                    Ok("CONTROLLED_TWO_OUTCOME_READING".into())
                },
                |event| {
                    if event["committed"] == true {
                        committed = true;
                    }
                    Ok(())
                },
                SeedMode::EitherValidOutcome,
            );
            let accepted = submitted == decision;
            assert_eq!(result.is_ok(), accepted);
            assert_eq!(committed, accepted);
            assert_eq!(
                model.calls.len(),
                2 + seed_calls + 2 * usize::from(accepted)
            );
            for (index, call) in model.calls.iter().enumerate() {
                let text = serde_json::to_string(call).unwrap();
                assert_eq!(
                    text.contains("CONTROLLED_TWO_OUTCOME_READING"),
                    index == 2 + seed_calls
                );
            }
        }
    }

    for (seed_reply, seed_generate, calls) in [("{}".into(), "", 3), (good_reply(), "", 4)] {
        let mut local = Local::ready_notebook(true);
        enable(&mut local);
        let mut model = engine(3);
        model.replies[2] = seed_reply;
        model.replies[3] = seed_generate.into();
        assert!(compare_gated(
            &local.config,
            &sample(2),
            &[sample(3)],
            &mut model,
            |_| panic!("invalid seed reached review"),
            |_| Ok(()),
            SeedMode::EitherValidOutcome,
        )
        .is_err());
        assert_eq!(model.calls.len(), calls);
    }
}
