//! Exercise the actual CLI routing/configuration, not only the library simulator.
use std::{fs, path::Path, process::Command};

#[test]
fn offline_entrypoint_ignores_normal_environment_and_preserves_scenario_assertions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
    for name in ["blank-answer", "disagreement"] {
        let temp = std::env::temp_dir().join(format!("reader-cli-{}-{name}", std::process::id()));
        fs::create_dir_all(&temp).unwrap();
        let mut scenario: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(format!("{name}.json"))).unwrap()).unwrap();
        for page in scenario["pages"].as_array_mut().unwrap() {
            if let Some(image) = page.get_mut("image") {
                *image = root.join(image.as_str().unwrap()).to_str().unwrap().into();
            }
        }
        scenario["output"] = temp.join("output").to_str().unwrap().into();
        let path = temp.join("scenario.json");
        fs::write(&path, serde_json::to_vec(&scenario).unwrap()).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .current_dir(&temp)
            .args(["--simulate", path.to_str().unwrap(), "--log-level", "off"])
            .env("OPENAI_API_KEY", "")
            .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
            .env("READER_BUDDY_DEBUG_DUMP", "invalid-unused-normal-setting")
            .env("RUST_LOG", "trace")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{name}: {} (evidence {})",
            String::from_utf8_lossy(&result.stderr),
            temp.display()
        );
        assert!(result.stderr.is_empty(), "explicit off must override trace");
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.join("output/report.json")).unwrap()).unwrap();
        assert_eq!(report["assertion_failures"], serde_json::json!([]));
        // The maintained scenarios assert exact positive text and negative no-write policy.
        // Preserve all artifacts on failure; remove only this test's owned temp directory.
        fs::remove_dir_all(temp).unwrap();
    }
}

#[test]
fn invalid_normal_config_fails_before_device_access_and_does_not_echo_keys() {
    for extra in [
        vec!["--trigger-corner", "BAD"],
        vec!["--simulate", "unused.json"],
        vec!["--log-level", "invalid"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .args(["--api-key", "fixture-secret-never-print"])
            .args(extra)
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!error.contains("fixture-secret-never-print"));
        assert!(!error.contains("framebuffer"));
        assert!(!error.contains("/dev/input"));
    }
}
