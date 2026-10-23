//! Fixed read-only unit observation. Never controls a unit or invokes a shell.
use super::Status;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
pub(super) trait Observer: Send + Sync {
    fn inactive(&self) -> Result<(), Status>;
}
pub(super) struct System;
const ARGS: [&str; 8] = [
    "--no-pager",
    "--no-ask-password",
    "show",
    "reader-buddy.service",
    "--property=LoadState",
    "--property=ActiveState",
    "--property=SubState",
    "--property=MainPID",
];
impl Observer for System {
    fn inactive(&self) -> Result<(), Status> {
        observe(Path::new("/bin/systemctl"), &ARGS, Duration::from_secs(3))
    }
}
fn decode(b: &[u8]) -> Result<(), Status> {
    let text = std::str::from_utf8(b).map_err(|_| Status::Unavailable)?;
    let mut props = std::collections::BTreeMap::new();
    for line in text.lines() {
        let (k, v) = line.split_once('=').ok_or(Status::Unavailable)?;
        if !["LoadState", "ActiveState", "SubState", "MainPID"].contains(&k)
            || props.insert(k, v).is_some()
        {
            return Err(Status::Unavailable);
        }
    }
    if props.len() != 4 {
        return Err(Status::Unavailable);
    }
    let pid = props["MainPID"]
        .parse::<u32>()
        .map_err(|_| Status::Unavailable)?;
    if pid.to_string() != props["MainPID"] {
        return Err(Status::Unavailable);
    }
    if ![
        "dead",
        "running",
        "exited",
        "failed",
        "start",
        "start-pre",
        "start-post",
        "stop",
        "stop-post",
        "stop-sigterm",
        "stop-sigkill",
        "auto-restart",
        "reload",
        "condition",
        "cleaning",
        "final-sigterm",
        "final-sigkill",
    ]
    .contains(&props["SubState"])
    {
        return Err(Status::Unavailable);
    }
    if props["LoadState"] == "loaded"
        && props["ActiveState"] == "inactive"
        && props["SubState"] == "dead"
        && pid == 0
    {
        Ok(())
    } else if [
        "active",
        "inactive",
        "activating",
        "deactivating",
        "failed",
        "reloading",
        "maintenance",
        "refreshing",
    ]
    .contains(&props["ActiveState"])
        && ["loaded", "not-found", "error", "bad-setting", "masked"].contains(&props["LoadState"])
    {
        Err(Status::Busy)
    } else {
        Err(Status::Unavailable)
    }
}
fn observe(path: &Path, args: &[&str], timeout: Duration) -> Result<(), Status> {
    let mut child = Command::new(path)
        .args(args)
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("SYSTEMD_PAGER", "cat")
        .env("SYSTEMD_COLORS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| Status::Unavailable)?;
    let out = child.stdout.take().ok_or(Status::Unavailable)?;
    let err = child.stderr.take().ok_or(Status::Unavailable)?;
    let (tx, rx) = mpsc::channel();
    fn reader(
        mut input: impl Read + Send + 'static,
        tx: mpsc::Sender<(bool, std::io::Result<Vec<u8>>)>,
        stderr: bool,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let r = input.by_ref().take(4097).read_to_end(&mut b).map(|_| b);
            let _ = tx.send((stderr, r));
        })
    }
    let a = reader(out, tx.clone(), false);
    let b = reader(err, tx, true);
    let start = Instant::now();
    let mut streams = Vec::new();
    let mut result = None;
    while start.elapsed() < timeout {
        while let Ok(item) = rx.try_recv() {
            streams.push(item)
        }
        if streams
            .iter()
            .any(|(_, r)| r.as_ref().map_or(true, |v| v.len() > 4096))
        {
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) if streams.len() == 2 => {
                if status.success() {
                    result = Some(())
                }
                break;
            }
            Err(_) => break,
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if result.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    // systemctl show has no child helper/pager. Join only after its pipes close.
    let _ = a.join();
    let _ = b.join();
    result.ok_or(Status::Unavailable)?;
    let mut stdout = None;
    for (stderr, r) in streams {
        let data = r.map_err(|_| Status::Unavailable)?;
        if stderr {
            if !data.is_empty() {
                return Err(Status::Unavailable);
            }
        } else {
            stdout = Some(data)
        }
    }
    decode(&stdout.ok_or(Status::Unavailable)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_properties_and_states() {
        let good = b"LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\n";
        assert_eq!(decode(good), Ok(()));
        for bad in [
            b"".as_slice(),
            b"MainPID=0\n",
            b"LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nMainPID=0\n",
            b"LoadState=loaded\nActiveState=unexpected\nSubState=dead\nMainPID=0\n",
        ] {
            assert_eq!(decode(bad), Err(Status::Unavailable))
        }
        assert_eq!(
            decode(b"LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=1\n"),
            Err(Status::Busy)
        );
    }
}

#[cfg(test)]
mod child_controls {
    use super::*;
    fn selected(name: &str) -> bool {
        std::env::args().any(|arg| arg == name)
    }
    #[test]
    #[ignore]
    fn child_timeout() {
        if selected("conversation_maintenance::service::child_controls::child_timeout") {
            std::thread::sleep(Duration::from_secs(5));
        }
    }
    #[test]
    #[ignore]
    fn child_overflow() {
        if selected("conversation_maintenance::service::child_controls::child_overflow") {
            use std::io::Write;
            std::io::stdout().write_all(&[b'x'; 5000]).unwrap();
        }
    }
    #[test]
    fn synthetic_child_timeout_and_overflow_are_reaped() {
        let exe = std::env::current_exe().unwrap();
        for name in [
            "conversation_maintenance::service::child_controls::child_timeout",
            "conversation_maintenance::service::child_controls::child_overflow",
        ] {
            let start = Instant::now();
            let result = observe(
                &exe,
                &["--ignored", "--exact", name, "--nocapture"],
                Duration::from_millis(250),
            );
            assert_eq!(result, Err(Status::Unavailable));
            assert!(start.elapsed() < Duration::from_secs(3));
        }
    }
    #[test]
    fn fixed_unit_and_environment_are_not_user_selected() {
        assert_eq!(
            ARGS,
            [
                "--no-pager",
                "--no-ask-password",
                "show",
                "reader-buddy.service",
                "--property=LoadState",
                "--property=ActiveState",
                "--property=SubState",
                "--property=MainPID"
            ]
        );
        assert_eq!(
            decode(
                b"LoadState=loaded\nActiveState=inactive\nSubState=unknown-synthetic\nMainPID=0\n"
            ),
            Err(Status::Unavailable)
        );
    }
}
