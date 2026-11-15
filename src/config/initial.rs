//! Fixed-path, offline creation of the first saved configuration.
use anyhow::{ensure, Result};
use serde::Serialize;
use std::{io::Write, path::Path};

#[cfg(target_os = "linux")]
mod linux;

const FIXED_PATH: &str = "/home/root/.config/remarkable-buddies/config.json";

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Action {
    CheckInitial,
    CreateInitial,
}

#[cfg_attr(test, derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum State {
    Blocked,
    #[cfg(target_os = "linux")]
    Missing,
    #[cfg(target_os = "linux")]
    Present,
    #[cfg(target_os = "linux")]
    Created,
    #[cfg(target_os = "linux")]
    Unknown,
}

#[derive(Serialize)]
struct Report {
    report_version: u32,
    operation: Action,
    state: State,
}

pub(super) fn write_report(
    action: Action,
    selected: Option<&Path>,
    output: &mut impl Write,
) -> Result<()> {
    let state = if selected.is_some_and(|path| path.as_os_str() != FIXED_PATH) {
        State::Blocked
    } else {
        #[cfg(target_os = "linux")]
        {
            linux::run(action, Path::new(FIXED_PATH))
        }
        #[cfg(not(target_os = "linux"))]
        {
            State::Blocked
        }
    };
    let mut bytes = serde_json::to_vec(&Report {
        report_version: 1,
        operation: action,
        state,
    })?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= 4096,
        "initial configuration report exceeds bound"
    );
    output.write_all(&bytes)?;
    Ok(())
}
