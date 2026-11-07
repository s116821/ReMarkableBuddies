//! Explicitly admitted, single-process qualification fixture.
use super::*;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) const MODEL: &str = "gpt-6.1-sol";
const ENDPOINT: &str = "https://api.openai.com";
const PAGES: [u32; 4] = [24, 35, 168, 170];

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

fn approve(raw: &[u8], expected: &str) -> anyhow::Result<Review> {
    let value: Review = serde_json::from_slice(raw)?;
    anyhow::ensure!(
        value.seed_gate_sha256 == expected
            && value.decision == "confirm-supported-controlled-seed"
            && selection(&value.selection),
        "confirmation refused"
    );
    Ok(value)
}

struct Meter<'a> {
    inner: crate::OpenAI,
    root: &'a Path,
    calls: usize,
    committed: &'a std::cell::Cell<bool>,
}

impl LLMEngine for Meter<'_> {
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
            self.calls < if self.committed.get() { 14 } else { 8 },
            "provider call budget"
        );
        self.calls += 1;
        save(
            self.root,
            &format!("call-{}-started.json", self.calls),
            &json!({"call":self.calls}),
        )?;
        let result = self.inner.execute_with_progress(progress);
        save(
            self.root,
            &format!("call-{}-ended.json", self.calls),
            &json!({"call":self.calls,"returned":result.is_ok()}),
        )?;
        result
    }
}

fn inputs(root: &Path) -> anyhow::Result<([Vec<u8>; 4], String)> {
    let map: Value = serde_json::from_slice(&read(&root.join("png-map.json"), 8192)?)?;
    let map = map
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("invalid image map"))?;
    anyhow::ensure!(map.len() == PAGES.len(), "image count");
    let mut images = Vec::new();
    for (page, fact) in PAGES.into_iter().zip(map) {
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
    Ok((
        images
            .try_into()
            .map_err(|_| anyhow::anyhow!("image count"))?,
        digest(&read(&root.join("png-map.json"), 8192)?),
    ))
}

fn run(root: &Path) -> anyhow::Result<()> {
    let manifest = read(&root.join("runtime.json"), 16384)?;
    let pin: Value = serde_json::from_slice(&manifest)?;
    let authority: Admission =
        serde_json::from_slice(&read(&root.join("live-admission.json"), 4096)?)?;
    anyhow::ensure!(
        authority.manifest_sha256 == digest(&manifest)
            && selection(&authority.selection)
            && pin["model"] == MODEL
            && pin["endpoint"] == ENDPOINT
            && pin["executable_sha256"] == digest(&fs::read(std::env::current_exe()?)?),
        "live admission"
    );
    let ([seed, first, second, third], map_sha) = inputs(root)?;
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
        committed: &committed,
    };
    let mut cell = 0;
    let result = compare_gated(
        &config,
        &seed,
        &[first, second, third],
        &mut engine,
        |gate| {
            save(root, "seed-gate.json", &gate)?;
            let gate_sha = digest(&read(&root.join("seed-gate.json"), 262144)?);
            let until = Instant::now() + Duration::from_secs(600);
            while Instant::now() < until {
                if root.join("review.json").exists() {
                    let review = approve(&read(&root.join("review.json"), 4096)?, &gate_sha)?;
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
    run(&root)
}

#[test]
#[ignore = "requires local frozen Barton inputs"]
fn check_barton_inputs() -> anyhow::Result<()> {
    let root = PathBuf::from(
        std::env::var_os("RMB_BARTON_ROOT")
            .ok_or_else(|| anyhow::anyhow!("fixture root absent"))?,
    );
    inputs(&root)?;
    Ok(())
}

#[test]
fn review_rejects_unknown_or_stale_gate() {
    let valid = json!({"decision":"confirm-supported-controlled-seed","seed_gate_sha256":"exact",
        "selection":"https://linear.app/magentumdragon/issue/REM-26#comment-11111111-1111-1111-1111-111111111111"});
    let bytes = serde_json::to_vec(&valid).unwrap();
    assert!(approve(&bytes, "exact").is_ok());
    assert!(approve(&bytes, "stale").is_err());
    assert!(approve(b"{}", "exact").is_err());
}
