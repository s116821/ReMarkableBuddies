use buddy::{
    archive_maintenance::{MAX_INPUT, MAX_OUTPUT},
    config::Config,
    storage::Store,
};
use remarkable_reader_buddy as buddy;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};
#[path = "support/archive_fixture.rs"]
mod fixture;
use fixture::Fixture;

// Test-only retained-command custody, captured once for this test process:
// READER_BUDDY_ARCHIVE_TEST_COMMAND = absolute fs::canonicalize path (including
// the verbatim prefix on Windows), with no symlink/alias or extra arguments.
// READER_BUDDY_ARCHIVE_TEST_SHA256 = exactly 64 lowercase hexadecimal digits.
// READER_BUDDY_ARCHIVE_TEST_BYTES = positive canonical decimal executable size.
// All three must be present or all absent. Absent preserves Cargo's normal
// CARGO_BIN_EXE_reader-buddy behavior. CI may select a release binary retained
// BEFORE cargo test (which may unify dev-only SDK features), then ship those
// same retained bytes. Selecting a Cargo-built local binary only tests this
// mechanism; it does not establish shipping release/platform qualification.
const CUSTODY_ENV: [&str; 3] = [
    "READER_BUDDY_ARCHIVE_TEST_COMMAND",
    "READER_BUDDY_ARCHIVE_TEST_SHA256",
    "READER_BUDDY_ARCHIVE_TEST_BYTES",
];
static COMMAND_CUSTODY: OnceLock<Option<RetainedCommand>> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
struct CommandStamp {
    bytes: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, u32, i64, i64),
}
impl CommandStamp {
    fn capture(metadata: &fs::Metadata) -> Result<Self, &'static str> {
        if !metadata.is_file() {
            return Err("retained archive command must be a regular file");
        }
        Ok(Self {
            bytes: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|_| "command metadata unavailable")?,
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.mode(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        })
    }
}

#[derive(Debug)]
struct RetainedCommand {
    path: PathBuf,
    sha256: String,
    stamp: CommandStamp,
}
impl RetainedCommand {
    // Pure inputs let controls exercise invalid selectors without changing the
    // process environment while the actual command tests run concurrently.
    fn select(values: [Option<OsString>; 3]) -> Result<Option<Self>, &'static str> {
        let [path, sha256, bytes] = values;
        let (path, sha256, bytes) = match (path, sha256, bytes) {
            (None, None, None) => return Ok(None),
            (Some(path), Some(sha256), Some(bytes)) => (PathBuf::from(path), sha256, bytes),
            _ => return Err("retained archive command custody requires all three variables"),
        };
        let sha256 = sha256
            .into_string()
            .map_err(|_| "invalid command SHA-256")?;
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid command SHA-256");
        }
        let bytes = bytes.into_string().map_err(|_| "invalid command size")?;
        let count: u64 = bytes.parse().map_err(|_| "invalid command size")?;
        if count == 0 || count.checked_add(1).is_none() || count.to_string() != bytes {
            return Err("invalid command size");
        }
        let stamp = Self::path_stamp(&path)?;
        if stamp.bytes != count {
            return Err("retained archive command size mismatch");
        }
        let value = Self {
            path,
            sha256,
            stamp,
        };
        value.verify()?;
        Ok(Some(value))
    }

    fn path_stamp(path: &Path) -> Result<CommandStamp, &'static str> {
        if !path.is_absolute()
            || fs::canonicalize(path)
                .map_err(|_| "retained archive command unavailable")?
                .as_os_str()
                != path.as_os_str()
        {
            return Err("retained archive command path must be absolute and canonical");
        }
        CommandStamp::capture(
            &fs::symlink_metadata(path).map_err(|_| "retained archive command unavailable")?,
        )
    }

    fn verify(&self) -> Result<(), &'static str> {
        if Self::path_stamp(&self.path)? != self.stamp {
            return Err("retained archive command metadata changed");
        }
        let file =
            fs::File::open(&self.path).map_err(|_| "retained archive command unavailable")?;
        if CommandStamp::capture(
            &file
                .metadata()
                .map_err(|_| "command metadata unavailable")?,
        )? != self.stamp
        {
            return Err("retained archive command metadata changed");
        }
        let mut reader = (&file).take(self.stamp.bytes + 1);
        let mut hash = Sha256::new();
        let mut count = 0;
        let mut buffer = [0; 64 * 1024];
        loop {
            let n = reader
                .read(&mut buffer)
                .map_err(|_| "retained archive command unreadable")?;
            if n == 0 {
                break;
            }
            count += n as u64;
            hash.update(&buffer[..n]);
        }
        if count != self.stamp.bytes || format!("{:x}", hash.finalize()) != self.sha256 {
            return Err("retained archive command bytes changed");
        }
        if CommandStamp::capture(
            &file
                .metadata()
                .map_err(|_| "command metadata unavailable")?,
        )? != self.stamp
            || Self::path_stamp(&self.path)? != self.stamp
        {
            return Err("retained archive command metadata changed");
        }
        Ok(())
    }

    fn verify_program(&self, cmd: &Command) -> Result<(), &'static str> {
        if cmd.get_program() != self.path.as_os_str() {
            return Err("retained archive command substitution");
        }
        self.verify()
    }
}

fn retained_command() -> Option<&'static RetainedCommand> {
    COMMAND_CUSTODY
        .get_or_init(|| {
            RetainedCommand::select(CUSTODY_ENV.map(std::env::var_os))
                .expect("invalid retained archive command custody")
        })
        .as_ref()
}
fn verify_command(cmd: &Command) {
    if let Some(retained) = retained_command() {
        retained
            .verify_program(cmd)
            .expect("retained archive command custody lost");
    }
}

fn command(f: &Fixture) -> Command {
    let path = retained_command().map_or_else(
        || Path::new(env!("CARGO_BIN_EXE_reader-buddy")),
        |retained| retained.path.as_path(),
    );
    let mut cmd = Command::new(path);
    cmd.current_dir(&f.root)
        .env("REMARKABLE_BUDDIES_CONFIG", f.root.join("config.json"))
        .env("OPENAI_API_KEY", "PRIVATE_ENV_SENTINEL")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
        .env("RUST_LOG", "trace");
    cmd
}
fn output(mut cmd: Command, input: Option<&[u8]>) -> Output {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    verify_command(&cmd);
    let mut child = cmd.spawn().unwrap();
    let writer = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let mut pipe = child.stdin.take().unwrap();
        std::thread::spawn(move || pipe.write_all(&bytes))
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            verify_command(&cmd);
            panic!("archive CLI exceeded caller deadline")
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(writer) = writer {
        if let Err(error) = writer.join().unwrap() {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe)
        }
    }
    let result = child.wait_with_output().unwrap();
    verify_command(&cmd);
    result
}
fn run_bytes(f: &Fixture, operation: &str, bytes: &[u8], file: bool) -> Value {
    let mut cmd = command(f);
    cmd.args(["--archive-operation", operation, "--archive-input"]);
    if file {
        fs::write(f.root.join("input.json"), bytes).unwrap();
        cmd.arg(f.root.join("input.json"));
    } else {
        cmd.arg("-");
    }
    let out = output(cmd, if file { None } else { Some(bytes) });
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= MAX_OUTPUT && out.stdout.ends_with(b"\n"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_"));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["activation_performed"], false);
    assert_eq!(report["report_version"], 1);
    report
}
fn run(f: &Fixture, operation: &str, path: &Path) -> Value {
    run_bytes(
        f,
        operation,
        &serde_json::to_vec(&json!({"path":path})).unwrap(),
        false,
    )
}
fn assert_no_sentinels(path: &Path) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_sentinels(&path)
        } else {
            assert!(!String::from_utf8_lossy(&fs::read(&path).unwrap()).contains("PRIVATE_"));
        }
    }
}

// Small non-executed fixture files test custody refusal, not product behavior.
fn custody_values(path: &Path, bytes: &[u8]) -> [Option<OsString>; 3] {
    [
        Some(path.as_os_str().to_owned()),
        Some(buddy::storage::digest(bytes).into()),
        Some(bytes.len().to_string().into()),
    ]
}

#[test]
fn retained_command_selector_is_explicit_complete_and_closed() {
    assert!(RetainedCommand::select([None, None, None])
        .unwrap()
        .is_none());
    let f = Fixture::new();
    let path = f.root.join("retained-command");
    let bytes = b"retained fixture, never executed";
    fs::write(&path, bytes).unwrap();
    let path = fs::canonicalize(path).unwrap();
    let values = custody_values(&path, bytes);
    assert!(RetainedCommand::select(values.clone()).unwrap().is_some());
    for mask in 1..7 {
        let mut incomplete = values.clone();
        for (index, value) in incomplete.iter_mut().enumerate() {
            if mask & (1 << index) == 0 {
                *value = None;
            }
        }
        assert!(RetainedCommand::select(incomplete).is_err());
    }
    for hash in [
        "".to_owned(),
        "a".repeat(63),
        "A".repeat(64),
        "g".repeat(64),
        format!("{} ", buddy::storage::digest(bytes)),
    ] {
        let mut invalid = values.clone();
        invalid[1] = Some(hash.into());
        assert!(RetainedCommand::select(invalid).is_err());
    }
    for size in [
        "",
        "0",
        "01",
        "+1",
        " 1",
        "1 ",
        "-1",
        "1.0",
        "18446744073709551615",
        "18446744073709551616",
    ] {
        let mut invalid = values.clone();
        invalid[2] = Some(size.into());
        assert!(RetainedCommand::select(invalid).is_err());
    }
    for (index, value) in [(1, "0".repeat(64)), (2, (bytes.len() + 1).to_string())] {
        let mut invalid = values.clone();
        invalid[index] = Some(value.into());
        assert!(RetainedCommand::select(invalid).is_err());
    }
    // Joining onto a Windows verbatim path can remove the dot component.
    let separator = std::path::MAIN_SEPARATOR;
    let mut dot_path = path.parent().unwrap().as_os_str().to_os_string();
    dot_path.push(format!("{separator}.{separator}"));
    dot_path.push(path.file_name().unwrap());
    assert_ne!(dot_path.as_os_str(), path.as_os_str());
    for invalid_path in [
        PathBuf::new(),
        PathBuf::from("retained-command"),
        path.with_file_name("missing"),
        path.parent().unwrap().to_path_buf(),
        PathBuf::from(dot_path),
    ] {
        assert!(RetainedCommand::select(custody_values(&invalid_path, bytes)).is_err());
    }
}

#[test]
fn retained_command_custody_refuses_substitution_mutation_and_removal() {
    let f = Fixture::new();
    let path = f.root.join("retained-command");
    let bytes = b"retained fixture, never executed";
    fs::write(&path, bytes).unwrap();
    let path = fs::canonicalize(path).unwrap();
    let retained = RetainedCommand::select(custody_values(&path, bytes))
        .unwrap()
        .unwrap();
    let cmd = Command::new(&path);
    retained.verify_program(&cmd).unwrap();
    assert!(retained
        .verify_program(&Command::new("reader-buddy"))
        .is_err());
    assert!(retained
        .verify_program(&Command::new(env!("CARGO_BIN_EXE_reader-buddy")))
        .is_err());
    // Same-length changed bytes cannot be admitted by a mere size check.
    fs::write(&path, vec![b'!'; bytes.len()]).unwrap();
    assert!(retained.verify_program(&cmd).is_err());
    assert!(RetainedCommand::select(custody_values(&path, bytes)).is_err());
    // A nonregular replacement or removal cannot recapture command custody.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(retained.verify_program(&cmd).is_err());
    fs::remove_dir(&path).unwrap();
    assert!(retained.verify_program(&cmd).is_err());
}

#[cfg(unix)]
#[test]
fn retained_command_custody_refuses_symlinks_fifo_and_same_bytes_replacement() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let path = f.root.join("retained-command");
    let bytes = b"retained fixture, never executed";
    fs::write(&path, bytes).unwrap();
    let path = fs::canonicalize(path).unwrap();
    let retained = RetainedCommand::select(custody_values(&path, bytes))
        .unwrap()
        .unwrap();
    let alias = path.with_file_name("command-alias");
    symlink(&path, &alias).unwrap();
    assert!(RetainedCommand::select(custody_values(&alias, bytes)).is_err());
    let saved = path.with_file_name("saved-command");
    fs::rename(&path, &saved).unwrap();
    symlink(&saved, &path).unwrap();
    assert!(retained.verify().is_err());
    fs::remove_file(&path).unwrap();
    fs::write(&path, bytes).unwrap();
    assert!(retained.verify().is_err());
    assert!(RetainedCommand::select(custody_values(&path, bytes))
        .unwrap()
        .is_some());
    fs::remove_file(&path).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    assert!(retained.verify().is_err());
    assert!(RetainedCommand::select(custody_values(&path, bytes)).is_err());
}

#[test]
fn real_selected_export_inspection_preserves_history_media_unrelated_data_and_exclusions() {
    for partial in [false, true] {
        let f = Fixture::new();
        let (store, scope) = f.selected(partial);
        let token = store
            .selected_snapshot(&scope, 4096)
            .unwrap()
            .unwrap()
            .token;
        drop(store);
        fs::write(
            f.root.join(".env"),
            b"OPENAI_API_KEY=PRIVATE_DOTENV_SENTINEL\nREADER_BUDDY_DEBUG_DUMP=poison\n",
        )
        .unwrap();
        let before = f.inventory();
        let destination = f.root.join("archive");
        let exported = run(&f, "export-selected", &destination);
        assert_eq!(exported["status"], "ok");
        assert_eq!(exported["incomplete"], partial);
        let inspected = run(&f, "inspect", &destination);
        assert_eq!(inspected["status"], "ok");
        assert_eq!(inspected["data"], exported["data"]);
        assert_eq!(inspected["data"]["inspection"]["format"], 2);
        assert_eq!(
            inspected["data"]["inspection"]["configuration_included"],
            false
        );
        assert_eq!(inspected["data"]["inspection"]["complete_media"], !partial);
        assert!(
            inspected["data"]["inspection"]["selection_receipts"]
                .as_u64()
                .unwrap()
                >= 2
        );
        assert_eq!(f.inventory(), before);
        assert_no_sentinels(&destination);
        let archive: Value =
            serde_json::from_slice(&fs::read(destination.join("backup.json")).unwrap()).unwrap();
        assert!(archive["manifests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|manifest| manifest["record_namespaces"]
                .as_object()
                .unwrap()
                .values()
                .any(|namespace| namespace == "source")));
        let historical = Store::inspect_selected_backup(&destination, &scope, 4096)
            .unwrap()
            .unwrap();
        assert_eq!(historical.selected_records.len(), 1);
        assert_eq!(historical.retained_records.len(), 1);
        assert_eq!(historical.media.is_empty(), partial);
        let store = Store::open(f.paths()).unwrap();
        assert_eq!(
            store
                .selected_snapshot(&scope, 4096)
                .unwrap()
                .unwrap()
                .token,
            token
        );
        assert!(store.restore(&destination).is_err());
        assert_eq!(f.inventory(), before);
        assert!(store.export(&f.root.join("legacy-refusal")).is_err());
        assert!(!f.root.join("legacy-refusal/backup.json").exists());
    }
}

#[test]
fn inspector_accepts_verified_legacy_config_presence_without_loading_config_or_opening_store() {
    let source = Fixture::new();
    let store = Store::open(source.paths()).unwrap();
    let config = Config::load(Some(source.root.join("config.json"))).unwrap();
    store.snapshot_config(&config).unwrap();
    let archive = source.root.join("legacy-archive");
    store.export(&archive).unwrap();
    drop(store);
    let f = Fixture::new();
    fs::write(f.root.join("config.json"), b"PRIVATE_INVALID_CONFIG").unwrap();
    fs::write(
        f.root.join(".env"),
        b"READER_BUDDY_DEBUG_DUMP=PRIVATE_POISON",
    )
    .unwrap();
    let report = run(&f, "inspect", &archive);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["data"]["inspection"]["format"], 1);
    assert_eq!(report["data"]["inspection"]["configuration_included"], true);
    assert!(f.inventory().is_empty());
    let missing = f.root.join("missing");
    assert_eq!(run(&f, "inspect", &missing)["status"], "unavailable");
    assert!(!missing.exists());
    assert!(f.inventory().is_empty());
}

#[test]
fn bounded_strict_file_stdin_input_and_actual_option_conflicts_refuse_before_storage() {
    let f = Fixture::new();
    let target = f.root.join("missing");
    let request = serde_json::to_vec(&json!({"path":target})).unwrap();
    let mut exact = request.clone();
    exact.resize(MAX_INPUT, b' ');
    assert_eq!(
        run_bytes(&f, "inspect", &exact, true),
        run_bytes(&f, "inspect", &exact, false)
    );
    exact.push(b' ');
    for file in [false, true] {
        assert_eq!(
            run_bytes(&f, "export-selected", &exact, file)["status"],
            "invalid-input"
        );
    }
    for request in [
        json!({"path":target,"unknown":"PRIVATE_SENTINEL"}),
        json!({"path":"relative"}),
        json!({"path":42}),
        json!({}),
        json!({"path":null}),
    ] {
        assert_eq!(
            run_bytes(
                &f,
                "export-selected",
                &serde_json::to_vec(&request).unwrap(),
                false
            )["status"],
            "invalid-input"
        );
    }
    assert_eq!(
        run_bytes(&f, "export-selected", b"{malformed", false)["status"],
        "invalid-input"
    );
    let mut cmd = command(&f);
    cmd.args([
        "--archive-operation",
        "inspect",
        "--archive-input",
        "relative.json",
    ]);
    let out = output(cmd, None);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    let mut cmd = command(&f);
    cmd.args(["--archive-operation", "inspect", "--archive-input"])
        .arg(&f.root);
    let out = output(cmd, None);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    for extra in [
        vec!["--model", "PRIVATE_MODEL"],
        vec!["--simulate", "PRIVATE_PATH"],
        vec!["--log-level", "trace"],
        vec!["--memory-operation", "query", "--memory-input", "-"],
        vec!["--sync-mode-operation", "inspect"],
        vec!["--personalization-operation", "inspect"],
        vec!["--export-note"],
        vec!["--inspect-owned-data"],
    ] {
        let mut cmd = command(&f);
        cmd.args(["--archive-operation", "inspect", "--archive-input", "-"])
            .args(extra);
        let out = output(cmd, None);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE_"));
    }
    for args in [
        vec!["--archive-operation", "inspect"],
        vec!["--archive-input", "-"],
    ] {
        let mut cmd = command(&f);
        cmd.args(args);
        assert!(!output(cmd, None).status.success());
    }
    assert!(f.inventory().is_empty());
    assert!(!target.exists());
}

#[test]
fn export_busy_missing_partial_legacy_and_destination_refusals_preserve_bytes() {
    let f = Fixture::new();
    let destination = f.root.join("archive");
    assert_eq!(
        run(&f, "export-selected", &destination)["status"],
        "unavailable"
    );
    assert!(f.inventory().is_empty());
    let (store, _) = f.selected(false);
    let before = f.inventory();
    assert_eq!(run(&f, "export-selected", &destination)["status"], "busy");
    assert_eq!(f.inventory(), before);
    assert!(!destination.exists());
    drop(store);
    let owner = f.paths().cache.join("owner.json");
    let saved = f.paths().cache.join("saved-owner");
    fs::rename(&owner, &saved).unwrap();
    let before = f.inventory();
    assert_eq!(
        run(&f, "export-selected", &destination)["status"],
        "unavailable"
    );
    assert_eq!(f.inventory(), before);
    fs::rename(saved, owner).unwrap();
    let existing = f.root.join("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("keep"), b"keep").unwrap();
    let before = f.inventory();
    for target in [
        existing.clone(),
        f.root.clone(),
        f.paths().data.join("nested"),
        f.paths().cache.join("nested"),
        f.paths().credentials.join("nested"),
    ] {
        assert_eq!(
            run(&f, "export-selected", &target)["status"],
            "invalid-input"
        );
        assert_eq!(f.inventory(), before);
    }
    assert_eq!(fs::read(existing.join("keep")).unwrap(), b"keep");
    f.save_config(true);
    assert_eq!(
        run(&f, "export-selected", &destination)["status"],
        "unsupported"
    );
    assert_eq!(f.inventory(), before);
    assert!(!destination.exists());
    let ordinary = Fixture::new();
    let store = Store::open(ordinary.paths()).unwrap();
    drop(store);
    let before = ordinary.inventory();
    let target = ordinary.root.join("archive");
    assert_eq!(
        run(&ordinary, "export-selected", &target)["status"],
        "failed"
    );
    assert_eq!(ordinary.inventory(), before);
    assert!(!target.exists());
}

#[test]
fn complete_archive_corruption_and_unknown_format_never_become_empty_success() {
    let f = Fixture::new();
    let (store, _) = f.selected(false);
    let target = f.root.join("archive");
    store.export_selected(&target).unwrap();
    drop(store);
    let marker = target.join("backup.json");
    let original = fs::read(&marker).unwrap();
    let mut value: Value = serde_json::from_slice(&original).unwrap();
    value["format"] = json!(999);
    fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();
    let before = fs::read(&marker).unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    assert_eq!(fs::read(&marker).unwrap(), before);
    fs::write(&marker, &original).unwrap();
    let object = fs::read_dir(target.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let bytes = fs::read(&object).unwrap();
    let missing = target.join("temporarily-missing-object");
    fs::rename(&object, &missing).unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    fs::rename(&missing, &object).unwrap();
    fs::write(&object, b"PRIVATE_CORRUPT_OBJECT").unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    assert_eq!(fs::read(&object).unwrap(), b"PRIVATE_CORRUPT_OBJECT");
    fs::write(object, bytes).unwrap();
    assert_eq!(run(&f, "inspect", &target)["status"], "ok");
    assert_eq!(fs::read(marker).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn fifo_input_archive_marker_and_store_lock_refuse_without_blocking() {
    let f = Fixture::new();
    let fifo = f.root.join("input-fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut cmd = command(&f);
    cmd.args(["--archive-operation", "inspect", "--archive-input"])
        .arg(&fifo);
    let out = output(cmd, None);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"],
        "invalid-input"
    );
    let target = f.root.join("archive");
    fs::create_dir(&target).unwrap();
    assert!(Command::new("mkfifo")
        .arg(target.join("backup.json"))
        .status()
        .unwrap()
        .success());
    assert_eq!(run(&f, "inspect", &target)["status"], "unsupported");
    let data = f.paths().data;
    fs::create_dir(&data).unwrap();
    assert!(Command::new("mkfifo")
        .arg(data.join("store.lock"))
        .status()
        .unwrap()
        .success());
    assert_eq!(
        run(&f, "export-selected", &f.root.join("destination"))["status"],
        "unsupported"
    );
    assert!(!f.root.join("destination").exists());
    let regular = f.root.join("regular-input.json");
    fs::write(
        &regular,
        serde_json::to_vec(&json!({"path":target})).unwrap(),
    )
    .unwrap();
    let linked = f.root.join("linked-input.json");
    std::os::unix::fs::symlink(&regular, &linked).unwrap();
    let mut cmd = command(&f);
    cmd.args(["--archive-operation", "inspect", "--archive-input"])
        .arg(&linked);
    assert_eq!(
        serde_json::from_slice::<Value>(&output(cmd, None).stdout).unwrap()["status"],
        "invalid-input"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn notebook_archive_cli_exports_product_generated_records_and_inspects_detached() {
    let f = Fixture::new();
    let maintained =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/simulator/scenarios");
    let mut scenario: Value = serde_json::from_slice(
        &fs::read(maintained.join("writer-notebook-create-replay.json")).unwrap(),
    )
    .unwrap();
    scenario["output"] = json!("output");
    scenario["retain_store_fixture"] = json!(true);
    fs::copy(
        maintained.join("writer-notebook-source.json"),
        f.root.join("writer-notebook-source.json"),
    )
    .unwrap();
    let input = f.root.join("scenario.json");
    fs::write(&input, serde_json::to_vec(&scenario).unwrap()).unwrap();
    let mut simulate = command(&f);
    simulate.arg("--simulate").arg(&input);
    let out = output(simulate, None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let scenario_report: Value =
        serde_json::from_slice(&fs::read(f.root.join("output/report.json")).unwrap()).unwrap();
    assert_eq!(scenario_report["model_calls"], 3);
    assert_eq!(scenario_report["store_fixture_saved"], true);

    // The simulator deliberately exports data only. Explicit fixture preparation
    // validates/rebinds that data before any archive command is exercised.
    let paths = buddy::storage::StorePaths {
        data: f.root.join("output/store-fixture/data"),
        ..f.paths()
    };
    let config = f.root.join("config.json");
    let store = Store::prepare_notebook(paths.clone()).unwrap();
    let manifests = store.manifests().unwrap();
    let conversation = serde_json::from_value(scenario["conversation"].clone()).unwrap();
    let ledger = buddy::conversation::Ledger::new(std::sync::Arc::new(store));
    let records = ledger.inspect(conversation, false).unwrap();
    assert!(records
        .iter()
        .any(|r| matches!(r, buddy::conversation::Record::ArchivedNormalizedSource(_))));
    assert!(records
        .iter()
        .any(|r| matches!(r, buddy::conversation::Record::NotebookPlan(_))));
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, buddy::conversation::Record::NotebookStage(_)))
            .count(),
        3
    );
    drop(ledger);
    fs::write(
        &config,
        serde_json::to_vec(&json!({"paths":paths})).unwrap(),
    )
    .unwrap();
    let current = fs::read(paths.data.join("CURRENT")).unwrap();
    let generation: String = serde_json::from_slice(&current).unwrap();
    let source = paths.data.join("generations").join(&generation);
    let read_inventory = |directory: &Path| {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_owned(),
                    fs::read(path).unwrap(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let objects = read_inventory(&source.join("objects"));
    let commits = read_inventory(&source.join("commits"));
    let archive = f.root.join("notebook-archive");
    let exported = run(&f, "export-notebook", &archive);
    assert_eq!(exported["status"], "ok");
    let summary = &exported["data"]["inspection"];
    assert_eq!(summary["format"], 4);
    assert_eq!(summary["complete_media"], true);
    assert_eq!(summary["selected_scopes"], 0);
    assert_eq!(summary["selection_receipts"], 0);
    assert_eq!(summary["manifests"], manifests.len());
    let marker = fs::read(archive.join("backup.json")).unwrap();
    let backup: Value = serde_json::from_slice(&marker).unwrap();
    assert_eq!(backup["origin_generation"], generation);
    let identity: Value =
        serde_json::from_slice(&fs::read(paths.data.join("identity.json")).unwrap()).unwrap();
    assert_eq!(backup["origin_actor"], identity["actor"]);
    assert_eq!(
        backup["manifests"],
        serde_json::to_value(manifests).unwrap()
    );
    let archived = read_inventory(&archive.join("objects"));
    for (name, bytes) in &archived {
        assert_eq!(objects.get(name), Some(bytes));
    }
    assert_eq!(
        archived.len(),
        summary["objects"].as_u64().unwrap() as usize
    );
    assert_eq!(
        run(&f, "export-notebook", &archive)["status"],
        "invalid-input"
    );
    assert_eq!(fs::read(archive.join("backup.json")).unwrap(), marker);
    fs::write(config, b"{poison config").unwrap();
    let detached = run(&f, "inspect-notebook", &archive);
    assert_eq!(detached["status"], "ok");
    assert_eq!(detached["data"], exported["data"]);
    assert_eq!(run(&f, "inspect", &archive)["status"], "unsupported");
    fs::write(archive.join("objects/undeclared"), b"extra").unwrap();
    assert_eq!(
        run(&f, "inspect-notebook", &archive)["status"],
        "unsupported"
    );
    fs::remove_file(archive.join("objects/undeclared")).unwrap();
    let (name, bytes) = archived.into_iter().next().unwrap();
    let object = archive.join("objects").join(name);
    fs::remove_file(&object).unwrap();
    assert_eq!(
        run(&f, "inspect-notebook", &archive)["status"],
        "unsupported"
    );
    let mut damaged = bytes;
    damaged[0] ^= 1;
    fs::write(object, damaged).unwrap();
    assert_eq!(
        run(&f, "inspect-notebook", &archive)["status"],
        "unsupported"
    );
    assert_eq!(fs::read(paths.data.join("CURRENT")).unwrap(), current);
    assert_eq!(read_inventory(&source.join("objects")), objects);
    assert_eq!(read_inventory(&source.join("commits")), commits);
}

#[test]
fn notebook_archive_cli_refuses_busy_wrong_format_and_incomplete_admission() {
    let f = Fixture::new();
    let path = f.root.join("destination");
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unavailable");
    assert!(!f.paths().data.exists());
    let legacy = Store::open(f.paths()).unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "busy");
    let legacy_archive = f.root.join("legacy");
    legacy.export(&legacy_archive).unwrap();
    use buddy::storage::{sync::mode::SyncModeChoice, Uuid};
    let mode = f.paths().data.join("sync/mode.json");
    legacy
        .set_sync_mode(
            &legacy.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            SyncModeChoice::IsolatedBackup {
                binding_sha256: buddy::storage::digest(b"archive fixture"),
            },
        )
        .unwrap();
    let nonlocal_mode = fs::read(&mode).unwrap();
    legacy
        .set_sync_mode(
            &legacy.sync_mode_snapshot().unwrap().token,
            Uuid::new_v4(),
            SyncModeChoice::LocalOnly,
        )
        .unwrap();
    let local_mode = fs::read(&mode).unwrap();
    drop(legacy);
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    assert_eq!(
        run(&f, "inspect-notebook", &legacy_archive)["status"],
        "unsupported"
    );
    let notebook = Store::prepare_notebook(f.paths()).unwrap();
    let current: String =
        serde_json::from_slice(&fs::read(f.paths().data.join("CURRENT")).unwrap()).unwrap();
    drop(notebook);
    let root = f.paths().data.join("generations").join(current);
    let capabilities = f.paths().data.join("capabilities.json");
    let original = fs::read(&capabilities).unwrap();
    fs::remove_file(&capabilities).unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::write(&capabilities, original).unwrap();
    fs::write(&mode, b"{").unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::write(&mode, nonlocal_mode).unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::write(&mode, local_mode).unwrap();
    let history = root.join("selection-history");
    fs::create_dir(&history).unwrap();
    fs::write(history.join("retained"), b"negative fixture").unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    fs::remove_file(history.join("retained")).unwrap();
    let local_archive = f.root.join("local-only");
    let exported = run(&f, "export-notebook", &local_archive);
    assert_eq!(exported["status"], "ok");
    let inspected = run(&f, "inspect-notebook", &local_archive);
    assert_eq!(inspected["status"], "ok");
    assert_eq!(inspected["data"], exported["data"]);
    fs::write(root.join("format.json"), b"1").unwrap();
    assert_eq!(run(&f, "export-notebook", &path)["status"], "unsupported");
    assert!(!path.exists());
}

#[cfg(windows)]
#[test]
fn notebook_archive_cli_refuses_case_aliases_without_changing_owned_paths() {
    fn snapshot(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
        let mut result = std::collections::BTreeMap::new();
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                result.extend(snapshot(&path));
                result.insert(path, None);
            } else {
                result.insert(path.clone(), Some(fs::read(path).unwrap()));
            }
        }
        result
    }

    let f = Fixture::new();
    let paths = f.paths();
    drop(Store::prepare_notebook(paths.clone()).unwrap());
    let before = snapshot(&paths.data);
    let alias = f.root.join("DATA");
    assert_ne!(alias, paths.data);
    assert_eq!(
        fs::read(alias.join("CURRENT")).unwrap(),
        fs::read(paths.data.join("CURRENT")).unwrap()
    );
    let destination = alias.join("new-archive");
    assert_eq!(
        run(&f, "export-notebook", &destination)["status"],
        "invalid-input"
    );
    assert!(!destination.exists());

    let mut aliased = paths.clone();
    aliased.cache = alias.join("cache");
    fs::write(
        f.root.join("config.json"),
        serde_json::to_vec(&json!({"paths":aliased})).unwrap(),
    )
    .unwrap();
    let destination = f.root.join("external-archive");
    assert_eq!(
        run(&f, "export-notebook", &destination)["status"],
        "invalid-input"
    );
    assert!(!destination.exists());
    assert_eq!(snapshot(&paths.data), before);
}

// The isolated exporter is crate-private and is already covered with its real
// producer in archive_maintenance::tests. This external CLI fixture uses the
// real public ordinary and portable exporters, then assembles only the fixed
// V1 outer marker. It does not qualify isolated lease/publication custody.
fn isolated_cli_package(source: &Fixture, partial: bool) -> (std::path::PathBuf, String, Value) {
    use buddy::storage::{
        digest, Coverage, Envelope, Kind, Manifest, Media, Namespace, ObjectRef, Omission, Scope,
        Uuid,
    };
    use std::collections::BTreeMap;
    let store = Store::open(source.paths()).unwrap();
    let media = if partial {
        b"PRIVATE_OMITTED_MEDIA".as_slice()
    } else {
        b"".as_slice()
    };
    let media_sha = digest(media);
    let entry = Envelope {
        envelope_version: 1,
        namespace: Namespace::Conversation,
        domain_schema_version: 1,
        record_id: Uuid::new_v4(),
        revision_id: Uuid::new_v4(),
        parents: Default::default(),
        operation_id: Uuid::new_v4(),
        actor_id: store.actor_id,
        kind: Kind::Value,
        payload: json!({"text":"PRIVATE_CONVERSATION"}),
        media_descriptors: vec![Media {
            sha256: media_sha.clone(),
            bytes: media.len() as u64,
            media_type: "application/octet-stream".into(),
        }],
    };
    if partial {
        let bytes = serde_json::to_vec(&entry).unwrap();
        let sha = digest(&bytes);
        store
            .import(
                Manifest {
                    format: 1,
                    transaction_id: Uuid::new_v4(),
                    scope: Scope::LocalTransaction,
                    records: vec![ObjectRef {
                        sha256: sha.clone(),
                        bytes: bytes.len() as u64,
                    }],
                    record_namespaces: BTreeMap::from([(sha.clone(), Namespace::Conversation)]),
                    media: vec![],
                    media_coverage: vec![Coverage::Omitted {
                        sha256: media_sha,
                        reason: Omission::SizeDeferred,
                    }],
                },
                BTreeMap::from([(sha, bytes)]),
            )
            .unwrap();
    } else {
        store
            .commit(vec![entry], BTreeMap::from([(media_sha, media.to_vec())]))
            .unwrap();
    }
    fs::write(
        source.root.join("config.json"),
        serde_json::to_vec(&json!({"paths":source.paths(),
        "personalization":{"enabled":true,"text":"PRIVATE_PORTABLE_TEXT"},"model":"PRIVATE_MODEL"}))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        source.paths().credentials.join("credential-sentinel"),
        b"PRIVATE_CREDENTIAL",
    )
    .unwrap();
    fs::write(
        source.paths().cache.join("cache-sentinel"),
        b"PRIVATE_CACHE",
    )
    .unwrap();
    let package = source.root.join("isolated-package");
    fs::create_dir(&package).unwrap();
    assert_eq!(store.export(&package.join("data")).unwrap(), !partial);
    drop(store);
    let mut exported = Vec::new();
    buddy::config::portable::export(source.root.join("config.json"))
        .write_to(&mut exported)
        .unwrap();
    let portable: Value = serde_json::from_slice(&exported).unwrap();
    assert_eq!(portable["status"], "ok");
    let artifact = serde_json::to_vec(&portable["artifact"]).unwrap();
    fs::write(package.join("portable-configuration.json"), &artifact).unwrap();
    let data = fs::read(package.join("data/backup.json")).unwrap();
    let data_ref = json!({"bytes":data.len(),"sha256":digest(&data)});
    let portable_ref = json!({"bytes":artifact.len(),"sha256":digest(&artifact)});
    let marker = serde_json::to_vec(&json!({"isolated_backup_version":1,"data_backup":data_ref,
        "portable_configuration":portable_ref}))
    .unwrap();
    fs::write(package.join("isolated-backup.json"), &marker).unwrap();
    let marker_sha = digest(&marker);
    let expected = json!({"marker_sha256":marker_sha,"data_backup":data_ref,"portable_configuration":portable_ref,
        "declared_settings":portable["artifact"]["settings"].as_object().unwrap().len(),
        "data":Store::inspect_backup(&package.join("data")).unwrap()});
    (package, marker_sha, expected)
}

fn package_inventory(
    root: &Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    let mut values = std::collections::BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            values.extend(package_inventory(&path));
            values.insert(path, None);
        } else {
            values.insert(path.clone(), Some(fs::read(&path).unwrap()));
        }
    }
    values
}

fn isolated_inspector() -> Fixture {
    let inspector = Fixture::new();
    fs::write(
        inspector.root.join("config.json"),
        b"PRIVATE_INVALID_CONFIG",
    )
    .unwrap();
    fs::write(
        inspector.root.join(".env"),
        b"OPENAI_API_KEY=PRIVATE_DOTENV_SENTINEL\nREADER_BUDDY_DEBUG_DUMP=PRIVATE_POISON\n",
    )
    .unwrap();
    inspector
}

fn assert_isolated_inspector_untouched(inspector: &Fixture, file_input: bool) {
    let names = fs::read_dir(&inspector.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    let mut expected = std::collections::BTreeSet::from([
        std::ffi::OsString::from(".env"),
        std::ffi::OsString::from("config.json"),
    ]);
    if file_input {
        expected.insert(std::ffi::OsString::from("input.json"));
    }
    assert_eq!(
        names, expected,
        "offline inspection created unexpected runtime files"
    );
    assert_eq!(
        fs::read(inspector.root.join("config.json")).unwrap(),
        b"PRIVATE_INVALID_CONFIG"
    );
    assert_eq!(
        fs::read(inspector.root.join(".env")).unwrap(),
        b"OPENAI_API_KEY=PRIVATE_DOTENV_SENTINEL\nREADER_BUDDY_DEBUG_DUMP=PRIVATE_POISON\n"
    );
    assert!(inspector.inventory().is_empty());
    for path in [
        inspector.paths().data,
        inspector.paths().cache,
        inspector.paths().credentials,
    ] {
        assert!(!path.exists());
    }
}

#[test]
fn isolated_archive_cli_checks_exact_stdin_and_file_without_runtime_or_owned_state() {
    for partial in [false, true] {
        let source = Fixture::new();
        let (package, marker, expected) = isolated_cli_package(&source, partial);
        let source_before = source.inventory();
        let package_before = package_inventory(&package);
        let inspector = isolated_inspector();
        let request = serde_json::to_vec(&json!({"path":package,"marker_sha256":marker})).unwrap();
        for file in [false, true] {
            let result = run_bytes(&inspector, "inspect-isolated", &request, file);
            assert_eq!(
                result,
                json!({"report_version":1,"operation":"inspect-isolated","status":"ok",
                "incomplete":partial,"activation_performed":false,"data":{"isolated_package":expected}})
            );
            assert!(!serde_json::to_string(&result)
                .unwrap()
                .contains(package.to_str().unwrap()));
            assert_isolated_inspector_untouched(&inspector, file);
            assert_eq!(source.inventory(), source_before);
            assert_eq!(package_inventory(&package), package_before);
        }
        let ordinary = run(&inspector, "inspect", &package.join("data"));
        assert_eq!(ordinary["status"], "ok");
        assert_eq!(ordinary["incomplete"], partial);
        assert_eq!(ordinary["data"], json!({"inspection":expected["data"]}));
        assert_eq!(
            fs::read(inspector.root.join("config.json")).unwrap(),
            b"PRIVATE_INVALID_CONFIG"
        );
    }
}

#[test]
fn isolated_archive_cli_refuses_wrong_marker_and_extra_members_without_changing_package() {
    let source = Fixture::new();
    let (package, marker, _) = isolated_cli_package(&source, false);
    let inspector = isolated_inspector();
    let request =
        |sha: &str| serde_json::to_vec(&json!({"path":package,"marker_sha256":sha})).unwrap();
    for file in [false, true] {
        let before = package_inventory(&package);
        let refused = run_bytes(
            &inspector,
            "inspect-isolated",
            &request(&buddy::storage::digest(b"wrong original")),
            file,
        );
        assert_eq!(refused["status"], "unsupported");
        assert!(refused["data"].is_null());
        assert_eq!(package_inventory(&package), before);
        for extra in [
            package.join("PRIVATE_EXTRA"),
            package.join("data/PRIVATE_EXTRA"),
            package.join("data/objects/PRIVATE_EXTRA"),
        ] {
            fs::write(&extra, b"PRIVATE_EXTRA_MEMBER").unwrap();
            let before = package_inventory(&package);
            let refused = run_bytes(&inspector, "inspect-isolated", &request(&marker), file);
            assert_eq!(refused["status"], "unsupported");
            assert!(refused["data"].is_null());
            assert_eq!(package_inventory(&package), before);
            fs::remove_file(extra).unwrap();
        }
        assert_isolated_inspector_untouched(&inspector, file);
    }
    assert!(inspector.inventory().is_empty());
}

#[test]
fn isolated_archive_cli_keeps_closed_input_caps_and_old_input_shapes() {
    let source = Fixture::new();
    let (package, marker, _) = isolated_cli_package(&source, false);
    let inspector = isolated_inspector();
    let request = serde_json::to_vec(&json!({"path":package,"marker_sha256":marker})).unwrap();
    let mut exact = request.clone();
    exact.resize(MAX_INPUT, b' ');
    for file in [false, true] {
        assert_eq!(
            run_bytes(&inspector, "inspect-isolated", &exact, file)["status"],
            "ok"
        );
        let mut oversized = exact.clone();
        oversized.push(b' ');
        assert_eq!(
            run_bytes(&inspector, "inspect-isolated", &oversized, file)["status"],
            "invalid-input"
        );
        for input in [
            json!({"path":package}),
            json!({"path":package,"marker_sha256":marker,"grant":"PRIVATE_GRANT"}),
            json!({"path":"relative","marker_sha256":marker}),
            json!({"path":package,"marker_sha256":"not-a-digest"}),
        ] {
            assert_eq!(
                run_bytes(
                    &inspector,
                    "inspect-isolated",
                    &serde_json::to_vec(&input).unwrap(),
                    file
                )["status"],
                "invalid-input"
            );
        }
        assert_eq!(
            run_bytes(&inspector, "inspect", &request, file)["status"],
            "invalid-input"
        );
        assert_isolated_inspector_untouched(&inspector, file);
    }
    assert!(inspector.inventory().is_empty());
}
