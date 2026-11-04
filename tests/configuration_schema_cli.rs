use remarkable_reader_buddy::{config::schema, storage::Uuid};
use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn actual_schema_selector_ignores_poisoned_inputs_and_exits_while_stdin_remains_open() {
    let root = std::env::temp_dir().join(format!("schema-cli-{}", Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join(".env"),b"REMARKABLE_BUDDIES_CONFIG=invalid\nWATCHDOG_USEC=poison\nOPENAI_API_KEY=PRIVATE_DOTENV_SENTINEL\n").unwrap();
    fs::write(root.join("poison.json"), b"PRIVATE_INVALID_CONFIG").unwrap();
    for file in ["missing.json", "poison.json"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reader-buddy"))
            .current_dir(&root)
            .arg("--configuration-schema")
            .env("REMARKABLE_BUDDIES_CONFIG", root.join(file))
            .env("OPENAI_API_KEY", "PRIVATE_ENV_SENTINEL")
            .env("OPENAI_BASE_URL", "poison")
            .env("READER_BUDDY_DEBUG_DUMP", "poison")
            .env("WATCHDOG_USEC", "poison")
            .env("NOTIFY_SOCKET", root.join("missing.sock"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let held_stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let reader = thread::spawn(move || {
            let mut out = Vec::new();
            stdout.read_to_end(&mut out).unwrap();
            out
        });
        let errors = thread::spawn(move || {
            let mut out = Vec::new();
            stderr.read_to_end(&mut out).unwrap();
            out
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("schema selector waited for input/runtime");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success());
        assert_eq!(reader.join().unwrap(), schema::DOCUMENT);
        assert!(errors.join().unwrap().is_empty());
        drop(held_stdin);
        let mut names = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, [".env", "poison.json"]);
        assert_eq!(
            fs::read(root.join("poison.json")).unwrap(),
            b"PRIVATE_INVALID_CONFIG"
        );
    }
    let resolved = root.canonicalize().unwrap();
    assert_eq!(
        resolved.parent(),
        Some(std::env::temp_dir().canonicalize().unwrap().as_path())
    );
    fs::remove_dir_all(resolved).unwrap();
}
