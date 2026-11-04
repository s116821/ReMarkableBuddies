use remarkable_reader_buddy::storage::{Store, StorePaths, Uuid};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reader-buddy"));
    command
        .current_dir(root)
        .env("REMARKABLE_BUDDIES_CONFIG", root.join("config.json"))
        .env_remove("NOTIFY_SOCKET")
        .env_remove("WATCHDOG_USEC")
        .arg("--inspect-selected-outbox");
    command
}

fn run_with_stdin_open(root: &Path) -> Vec<u8> {
    let mut child = command(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let held_stdin = child.stdin.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            child.wait().unwrap();
            panic!("inspection must exit with stdin held open");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    drop(held_stdin);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    output.stdout
}

#[test]
fn selected_outbox_cli_is_exclusive_and_reports_cross_process_busy_without_stdin() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("buddy-outbox-cli-{}", Uuid::new_v4())));
    let root = &fixture.0;
    fs::create_dir(root).unwrap();
    let paths = StorePaths {
        data: root.join("data"),
        cache: root.join("cache"),
        credentials: root.join("credentials"),
    };
    fs::write(
        root.join("config.json"),
        serde_json::to_vec(&json!({"paths":paths})).unwrap(),
    )
    .unwrap();

    for peer in [
        &["--configuration-schema"][..],
        &["--model", "fixture"],
        &["operand"],
    ] {
        let output = command(root).args(peer).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!paths.data.exists());
    }

    assert_eq!(
        run_with_stdin_open(root),
        b"{\"report_version\":1,\"operation\":\"inspect-selected-outbox\",\"status\":\"unavailable\",\"counts\":null}\n"
    );
    assert!(!paths.data.exists());
    let owner = Store::open(paths).unwrap();
    assert_eq!(
        run_with_stdin_open(root),
        b"{\"report_version\":1,\"operation\":\"inspect-selected-outbox\",\"status\":\"busy\",\"counts\":null}\n"
    );
    drop(owner);
    assert_eq!(
        run_with_stdin_open(root),
        b"{\"report_version\":1,\"operation\":\"inspect-selected-outbox\",\"status\":\"ok\",\"counts\":{\"pending\":0,\"withdrawn\":0}}\n"
    );
}
