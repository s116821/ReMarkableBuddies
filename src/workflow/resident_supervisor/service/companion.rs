//! Read-only system-bus ownership admission for the fixed vendor companion.
//! This never activates a service and is not Reader or native mutation authority.
use super::*;

const NAME: &str = "no.remarkable.sync";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerOwner {
    pub broker_id: String,
    pub unique_name: String,
    pub pid: u32,
}

/// Missing ownership is a startup observation, not permission to start the
/// original bound service. Transport failures and changed samples are errors.
pub fn observe_owner(
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> Result<Option<BrokerOwner>> {
    owner_with(
        deadline,
        progress,
        |method, argument, deadline, progress| {
            let mut command = Command::new("/usr/bin/busctl");
            command.args([
                "--system",
                "--timeout=2s",
                "call",
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                method,
            ]);
            if let Some(argument) = argument {
                command.args(["s", argument]);
            }
            run_with_progress(command, deadline, progress)
        },
    )
}

fn owner_with(
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
    mut call: impl FnMut(&str, Option<&str>, Instant, &mut dyn FnMut() -> Result<()>) -> Result<Vec<u8>>,
) -> Result<Option<BrokerOwner>> {
    let mut query = |method, argument| -> Result<Vec<u8>> {
        crate::workflow::resident_request::before(deadline, "companion broker deadline expired")?;
        progress()?;
        crate::workflow::resident_request::before(deadline, "companion broker deadline expired")?;
        let result = call(method, argument, deadline, progress)?;
        progress()?;
        crate::workflow::resident_request::before(deadline, "companion broker deadline expired")?;
        Ok(result)
    };
    match query("NameHasOwner", Some(NAME))?.as_slice() {
        b"b false\n" => return Ok(None),
        b"b true\n" => {}
        _ => anyhow::bail!("Malformed companion ownership presence"),
    }
    let broker_id = broker_id(&query("GetId", None)?)?;
    let unique_name = unique_name(&query("GetNameOwner", Some(NAME))?)?;
    let pid = process_id(&query("GetConnectionUnixProcessID", Some(&unique_name))?)?;
    ensure!(
        self::unique_name(&query("GetNameOwner", Some(NAME))?)? == unique_name
            && self::broker_id(&query("GetId", None)?)? == broker_id,
        "Companion broker ownership changed during observation"
    );
    Ok(Some(BrokerOwner {
        broker_id,
        unique_name,
        pid,
    }))
}

fn string_reply(bytes: &[u8]) -> Result<&str> {
    ensure!(bytes.len() <= 260, "Companion broker string exceeds bound");
    std::str::from_utf8(bytes)?
        .strip_prefix("s \"")
        .and_then(|value| value.strip_suffix("\"\n"))
        .context("Malformed companion broker string")
}
fn broker_id(bytes: &[u8]) -> Result<String> {
    let value = string_reply(bytes)?;
    ensure!(
        value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Malformed companion broker ID"
    );
    Ok(value.into())
}
fn unique_name(bytes: &[u8]) -> Result<String> {
    let value = string_reply(bytes)?;
    let components = value.strip_prefix(':').context("Non-unique broker name")?;
    ensure!(
        value.len() <= 255
            && components.contains('.')
            && components.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            }),
        "Malformed unique broker name"
    );
    Ok(value.into())
}
fn process_id(bytes: &[u8]) -> Result<u32> {
    ensure!(bytes.len() <= 13, "Companion broker PID exceeds bound");
    let text = std::str::from_utf8(bytes)?;
    let pid: u32 = text
        .strip_prefix("u ")
        .and_then(|value| value.strip_suffix('\n'))
        .context("Malformed companion broker PID")?
        .parse()?;
    ensure!(
        pid > 1 && text == format!("u {pid}\n"),
        "Invalid companion broker PID"
    );
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn samples() -> VecDeque<Vec<u8>> {
        [
            "b true\n",
            "s \"0123456789abcdef0123456789abcdef\"\n",
            "s \":1.42\"\n",
            "u 1234\n",
            "s \":1.42\"\n",
            "s \"0123456789abcdef0123456789abcdef\"\n",
        ]
        .map(|value| value.as_bytes().to_vec())
        .into()
    }

    #[test]
    fn owner_is_bracketed_and_pid_query_targets_the_unique_name() {
        let mut samples = samples();
        let mut calls = Vec::new();
        let owner = owner_with(
            Instant::now() + Duration::from_secs(1),
            &mut || Ok(()),
            |method, argument, _, _| {
                calls.push((method.to_owned(), argument.map(str::to_owned)));
                Ok(samples.pop_front().unwrap())
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(owner.pid, 1234);
        assert_eq!(calls.len(), 6);
        assert_eq!(
            calls[3],
            ("GetConnectionUnixProcessID".into(), Some(":1.42".into()))
        );
        assert!(samples.is_empty());
        assert!(!calls.iter().any(|(method, _)| method.contains("Start")));
    }

    #[test]
    fn missing_owner_is_not_activation_and_replacement_or_bad_output_refuses() {
        let mut calls = 0;
        assert!(owner_with(
            Instant::now() + Duration::from_secs(1),
            &mut || Ok(()),
            |_, _, _, _| {
                calls += 1;
                Ok(b"b false\n".to_vec())
            }
        )
        .unwrap()
        .is_none());
        assert_eq!(calls, 1);
        for (index, bad) in [
            (0, "true\n"),
            (1, "s \"not-a-broker-id\"\n"),
            (2, "s \"no.remarkable.sync\"\n"),
            (3, "u 01234\n"),
            (3, "u 1\n"),
            (4, "s \":1.43\"\n"),
            (5, "s \"1123456789abcdef0123456789abcdef\"\n"),
        ] {
            let mut values = samples();
            values[index] = bad.as_bytes().to_vec();
            assert!(
                owner_with(
                    Instant::now() + Duration::from_secs(1),
                    &mut || Ok(()),
                    |_, _, _, _| { Ok(values.pop_front().unwrap()) }
                )
                .is_err(),
                "{index}: {bad}"
            );
        }
    }

    #[test]
    fn cancellation_and_expiry_stop_further_queries() {
        let mut calls = 0;
        assert!(owner_with(
            Instant::now() + Duration::from_secs(1),
            &mut || anyhow::bail!("canceled"),
            |_, _, _, _| {
                calls += 1;
                Ok(Vec::new())
            }
        )
        .is_err());
        assert_eq!(calls, 0);
        assert!(owner_with(Instant::now(), &mut || Ok(()), |_, _, _, _| {
            calls += 1;
            Ok(Vec::new())
        })
        .is_err());
        assert_eq!(calls, 0);
        let deadline = Instant::now() + Duration::from_millis(5);
        assert!(owner_with(deadline, &mut || Ok(()), |_, _, _, _| {
            calls += 1;
            thread::sleep(Duration::from_millis(10));
            Ok(b"b true\n".to_vec())
        })
        .is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn post_query_cancellation_and_transport_errors_withhold_delivery() {
        let mut checks = 0;
        let mut calls = 0;
        let error = owner_with(
            Instant::now() + Duration::from_secs(1),
            &mut || {
                checks += 1;
                ensure!(checks == 1, "canceled after reply");
                Ok(())
            },
            |_, _, _, _| {
                calls += 1;
                Ok(b"b false\n".to_vec())
            },
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.to_string(), "canceled after reply");
        let error = owner_with(
            Instant::now() + Duration::from_secs(1),
            &mut || Ok(()),
            |_, _, _, _| anyhow::bail!("transport failed"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "transport failed");
    }
}
