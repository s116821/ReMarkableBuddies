use base64::{engine::general_purpose::STANDARD, Engine};
use remarkable_reader_buddy::{
    config::Config,
    storage::{StorePaths, Uuid},
};
use serde_json::{json, Value};
use std::{
    io::{Cursor, Write},
    path::PathBuf,
    process::{Command, Output, Stdio},
};
struct Local {
    root: PathBuf,
    config: Config,
    path: PathBuf,
}
impl Local {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rmb-handwriting-cli-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let config = Config {
            paths: StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            },
            ..Config::default()
        };
        let path = root.join("config.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        Self { root, config, path }
    }
    fn run(&self, bytes: &[u8], extra: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .arg("--handwriting")
            .args(extra)
            .env("REMARKABLE_BUDDIES_CONFIG", &self.path)
            .env("OPENAI_API_KEY", "unused-synthetic-canary")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // An early bounded refusal can legitimately close input before the caller finishes.
        let _ = child.stdin.take().unwrap().write_all(bytes);
        child.wait_with_output().unwrap()
    }
}
impl Drop for Local {
    fn drop(&mut self) {
        let p = self.root.canonicalize().unwrap();
        assert_eq!(
            p.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rmb-handwriting-cli-"));
        std::fs::remove_dir_all(p).unwrap();
    }
}
fn input() -> Value {
    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        1,
        image::Rgba([0, 0, 0, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    json!({"handwriting_version":1,"command":"confirm","profile":"11111111-1111-4111-8111-111111111111","correction":"22222222-2222-4222-8222-222222222222","operation":"33333333-3333-4333-8333-333333333333","expected_heads":[],"confirmation":"user-confirmed","original_text":"PRIVATE_ORIGINAL_1O","corrected_text":"PRIVATE_CORRECTED_10 m/s²\n","confirmed_ms":1,"provenance":{"kind":"caller-confirmed-sample","description":"Synthetic CLI test","source":null},"sample_png_base64":STANDARD.encode(bytes.into_inner())})
}
fn report(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.ends_with(b"\n"));
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn actual_cli_confirm_status_query_and_restart_keep_explicit_text_out_of_routine_logs() {
    let local = Local::new();
    let input = input();
    let bytes = serde_json::to_vec(&input).unwrap();
    let output = local.run(&bytes, &[]);
    let first = report(&output);
    assert_eq!(first["status"], "complete");
    assert_eq!(first["historical"], false);
    assert_eq!(
        first["facts"][0]["value"]["corrected_text"],
        input["corrected_text"]
    );
    assert!(first["facts"][0].get("sample_png_base64").is_none());
    for canary in [
        "PRIVATE_ORIGINAL",
        "PRIVATE_CORRECTED",
        "unused-synthetic-canary",
        "iVBOR",
    ] {
        assert!(!String::from_utf8_lossy(&output.stderr).contains(canary));
    }
    let status = json!({"handwriting_version":1,"command":"status","profile":input["profile"],"correction":input["correction"],"operation":input["operation"]});
    let read = report(&local.run(&serde_json::to_vec(&status).unwrap(), &[]));
    assert_eq!(read["historical"], true);
    assert!(read["transaction"].is_null());
    assert_eq!(read["diagnostics"]["mutation_outcome"], "observed");
    let replay = report(&local.run(&bytes, &[]));
    assert_eq!(replay["historical"], true);
    assert!(replay["transaction"].is_null());
    let query = json!({"handwriting_version":1,"command":"query","profile":input["profile"],"query_text":null,"max_matches":16,"max_encoded_bytes":65536});
    let read = report(&local.run(&serde_json::to_vec(&query).unwrap(), &[]));
    assert_eq!(read["facts"].as_array().unwrap().len(), 1);
    assert_eq!(read["diagnostics"]["mutation_outcome"], "not-requested");
}
#[test]
fn invalid_and_one_over_inputs_refuse_before_store_creation_and_bad_flags_before_stdin() {
    let local = Local::new();
    let mut v = input();
    v["confirmation"] = json!("guessed");
    let value = report(&local.run(&serde_json::to_vec(&v).unwrap(), &[]));
    assert_eq!(value["failure"], "invalid-confirmation");
    assert!(!local.config.paths.data.exists());
    let value = report(&local.run(&vec![b' '; 768 * 1024 + 1], &[]));
    assert_eq!(value["failure"], "input-limit");
    assert!(!local.config.paths.data.exists());
    for extra in [
        &["--writer-note"][..],
        &["--model", "not-a-model"][..],
        &["--inspect-owned-data"][..],
    ] {
        let output = local.run(b"", extra);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!local.config.paths.data.exists());
    }
}
#[test]
fn declared_legacy_sync_refuses_without_store_or_provider_startup() {
    let mut local = Local::new();
    local.config.sync.enabled = true;
    std::fs::write(&local.path, serde_json::to_vec(&local.config).unwrap()).unwrap();
    let value = report(&local.run(&serde_json::to_vec(&input()).unwrap(), &[]));
    assert_eq!(value["status"], "unsupported");
    assert_eq!(value["failure"], "unsupported-mode");
    assert!(!local.config.paths.data.exists());
}
