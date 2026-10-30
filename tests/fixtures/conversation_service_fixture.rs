//! Synthetic subprocess bytes only. Never observes or controls an actual unit.
use std::io::{Read, Write};
fn main() {
    for (key, value) in [
        ("LANG", "C"),
        ("LC_ALL", "C"),
        ("SYSTEMD_PAGER", "cat"),
        ("SYSTEMD_COLORS", "0"),
    ] {
        if std::env::var(key).as_deref() != Ok(value) {
            std::process::exit(31);
        }
    }
    #[cfg(unix)]
    if std::env::vars_os().count() != 4 {
        std::process::exit(32);
    }
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    if !input.is_empty() {
        std::process::exit(33);
    }
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "wait" {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    let body = if mode == "busy" {
        "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=1\n"
    } else {
        "LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\n"
    };
    std::io::stdout().write_all(body.as_bytes()).unwrap();
    match mode.as_str() {
        "stderr" => eprintln!("synthetic diagnostic"),
        "duplicate" => println!("MainPID=0"),
        "overflow" => std::io::stdout().write_all(&[b'x'; 4097]).unwrap(),
        "nonzero" => std::process::exit(17),
        _ => {}
    }
}
