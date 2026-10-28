//! Capture without initializing input devices or requiring provider credentials.
use remarkable_reader_buddy::device::screenshot::CaptureIdentity;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,remarkable_reader_buddy=debug"),
    )
    .format_timestamp_millis()
    .init();
    let (path, image_only, identity) = parse_args(std::env::args_os().skip(1))?;
    let mut screenshot = match identity {
        Some(identity) => remarkable_reader_buddy::Screenshot::new_for_process(identity)?,
        None => remarkable_reader_buddy::Screenshot::new()?,
    };
    if image_only {
        // Saving is outside capture.total, matching the owned-pixel status path.
        screenshot.take_image()?.save(path)?;
        return Ok(());
    }
    screenshot.take_screenshot()?;
    screenshot.save_image(&path)
}

fn parse_args(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> anyhow::Result<(String, bool, Option<CaptureIdentity>)> {
    let path = args.next().ok_or_else(|| {
        anyhow::anyhow!(
            "Usage: screenshot FILE [--image-only] [--pid PID --start-ticks TICKS --boot-id UUID]"
        )
    })?;
    let mut image_only = false;
    let (mut pid, mut start, mut boot) = (None, None, None);
    let (mut resident_nonce, mut executable_hash) = (None::<String>, None::<String>);
    while let Some(flag) = args.next() {
        let flag = flag
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid flag"))?;
        if flag == "--image-only" && !image_only {
            image_only = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing flag value"))?;
        let value = value
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid flag value"))?;
        match flag {
            "--pid" if pid.is_none() => pid = Some(value.parse::<u32>()?),
            "--start-ticks" if start.is_none() => start = Some(value.parse::<u64>()?),
            "--boot-id" if boot.is_none() => boot = Some(value.to_owned()),
            "--resident-nonce" if resident_nonce.is_none() => {
                resident_nonce = Some(value.to_owned())
            }
            "--executable-sha256" if executable_hash.is_none() => {
                executable_hash = Some(value.to_owned())
            }
            _ => anyhow::bail!("Unknown or duplicate screenshot flag"),
        }
    }
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Output path must be UTF-8"))?;
    anyhow::ensure!(
        resident_nonce.is_some() == executable_hash.is_some(),
        "Resident capture requires both nonce and executable hash"
    );
    let bind = |pid, start, boot| {
        if resident_nonce.is_some() {
            #[cfg(feature = "development-resident-bootstrap")]
            return CaptureIdentity::new_for_resident(
                pid,
                start,
                boot,
                resident_nonce.as_deref().unwrap(),
                executable_hash.as_deref().unwrap(),
            );
            #[cfg(not(feature = "development-resident-bootstrap"))]
            anyhow::bail!("Resident capture requires development-resident-bootstrap");
        }
        CaptureIdentity::new(pid, start, boot)
    };
    let identity = match (pid, start, boot) {
        (None, None, None) => None,
        (Some(pid), Some(start), Some(boot)) => Some(bind(pid, start, boot)?),
        _ => anyhow::bail!("Capture identity requires --pid, --start-ticks and --boot-id together"),
    };
    anyhow::ensure!(
        resident_nonce.is_none() || identity.is_some(),
        "Resident capture requires explicit process identity"
    );
    Ok((path.to_owned(), image_only, identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> anyhow::Result<(String, bool, Option<CaptureIdentity>)> {
        parse_args(values.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn helper_accepts_complete_binding_and_legacy_arguments() {
        assert!(parse(&["frame.png"]).unwrap().2.is_none());
        assert!(parse(&["frame.png", "--image-only"]).unwrap().1);
        let (_, image_only, identity) = parse(&[
            "frame.png",
            "--pid",
            "42",
            "--image-only",
            "--boot-id",
            "02b2347b-03aa-4a85-8dd7-d731aabf6f3e",
            "--start-ticks",
            "123",
        ])
        .unwrap();
        assert!(image_only && identity.is_some());
    }

    #[test]
    fn partial_duplicate_unknown_and_malformed_bindings_refuse_without_discovery() {
        for values in [
            vec![],
            vec!["frame.png", "--pid", "42"],
            vec!["frame.png", "--start-ticks", "123"],
            vec!["frame.png", "--boot-id", "bad"],
            vec!["frame.png", "--pid", "42", "--pid", "43"],
            vec!["frame.png", "--image-only", "--image-only"],
            vec!["frame.png", "--other", "42"],
            vec!["frame.png", "--pid", "not-number"],
            vec!["frame.png", "--pid"],
            vec!["frame.png", "--resident-nonce", "12"],
            vec![
                "frame.png",
                "--resident-nonce",
                "12121212121212121212121212121212",
                "--executable-sha256",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ],
            vec![
                "frame.png",
                "--pid",
                "0",
                "--start-ticks",
                "123",
                "--boot-id",
                "02b2347b-03aa-4a85-8dd7-d731aabf6f3e",
            ],
        ] {
            assert!(parse(&values).is_err(), "{values:?}");
        }
    }

    #[test]
    fn copied_reader_mode_requires_feature_and_complete_binding() {
        let result = parse(&[
            "frame.png",
            "--pid",
            "42",
            "--start-ticks",
            "123",
            "--boot-id",
            "02b2347b-03aa-4a85-8dd7-d731aabf6f3e",
            "--resident-nonce",
            "12121212121212121212121212121212",
            "--executable-sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ]);
        assert_eq!(
            result.is_ok(),
            cfg!(feature = "development-resident-bootstrap")
        );
    }
}
