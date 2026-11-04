use super::*;
fn bytes(units: &[&str]) -> Vec<u8> {
    units
        .iter()
        .map(|unit| {
            let mut fields: BTreeMap<String, String> = STATE
                .iter()
                .chain(POLICY)
                .chain(EXTRA)
                .map(|k| ((*k).into(), String::new()))
                .collect();
            for key in [
                "MainPID",
                "ControlPID",
                "ExecMainPID",
                "ExecMainStartTimestampMonotonic",
            ] {
                fields.insert(key.into(), "0".into());
            }
            fields.insert("Id".into(), (*unit).into());
            for (key, value) in [
                ("LoadState", "loaded"),
                ("ActiveState", "inactive"),
                ("SubState", "dead"),
            ] {
                fields.insert(key.into(), value.into());
            }
            if *unit == "xochitl.service" || *unit == "rm-sync.service" {
                for (key, value) in vendor::policy(unit) {
                    fields.insert(key.into(), value.into());
                }
            }
            fields
                .into_iter()
                .map(|(k, v)| format!("{k}={v}\n"))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}
#[test]
fn coherent_batches_keep_complete_vendor_policy_and_refuse_bad_shapes() {
    for units in [
        vec!["reader-buddy.service", "xochitl.service"],
        vec!["reader-buddy.service", "xochitl.service", "rm-sync.service"],
    ] {
        let raw = bytes(&units);
        let batch = ServiceBatch::parse(&units, &raw).unwrap();
        for unit in units
            .iter()
            .filter(|u| **u == "xochitl.service" || **u == "rm-sync.service")
        {
            let expected = vendor::policy(unit);
            vendor::check_policy(
                &batch.policy_bytes(unit, expected.keys().copied()).unwrap(),
                &expected,
            )
            .unwrap();
        }
        for bad in [
            raw[..raw.len() - 5].to_vec(),
            [raw.clone(), b"Id=foreign.service\n".to_vec()].concat(),
            raw.iter()
                .copied()
                .chain(std::iter::repeat_n(b'x', LIMIT))
                .collect(),
            raw.iter().copied().filter(|_| false).collect(),
        ] {
            assert!(ServiceBatch::parse(&units, &bad).is_err());
        }
        let mut missing = String::from_utf8(raw.clone()).unwrap();
        missing = missing
            .lines()
            .filter(|l| !l.starts_with("NotifyAccess="))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(ServiceBatch::parse(&units, missing.as_bytes()).is_err());
        assert!(ServiceBatch::parse(&["xochitl.service", "xochitl.service"], &raw).is_err());
    }
}
#[test]
fn batch_command_count_does_not_multiply_with_internal_poll_count() {
    for polls in [1, 1000] {
        let units = ["xochitl.service", "rm-sync.service"];
        let mut calls = 0;
        let mut pulses = 0;
        let result = observe_many_with(
            &units,
            Instant::now() + Duration::from_secs(2),
            &mut || {
                pulses += 1;
                Ok(())
            },
            |command, _, pulse| {
                calls += 1;
                assert_eq!(command.get_program(), "/bin/systemctl");
                for _ in 0..polls {
                    pulse()?;
                }
                Ok(bytes(&units))
            },
        );
        assert!(result.is_ok());
        assert_eq!(calls, 1);
        assert_eq!(pulses, polls);
        let mut attempts = 0;
        assert!(observe_many_with(
            &units,
            Instant::now() + Duration::from_secs(2),
            &mut || anyhow::bail!("cancel"),
            |_, _, pulse| {
                attempts += 1;
                pulse()?;
                Ok(bytes(&units))
            }
        )
        .is_err());
        assert_eq!(attempts, 1);
    }
}

impl ServiceBatch {
    pub(crate) fn fixture_observe(
        units: &[&str],
        deadline: Instant,
        polls: usize,
        count: &mut usize,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        observe_many_with(units, deadline, pulse, |command, deadline, pulse| {
            assert_eq!(command.get_program(), "/bin/systemctl");
            *count += 1;
            for _ in 0..polls {
                pulse()?;
            }
            let raw = String::from_utf8(bytes(units))?;
            let mut child = Command::new("/bin/sh");
            child.args(["-c", "printf '%s' \"$1\"", "pid1-probe", &raw]);
            run_with_progress(child, deadline, pulse)
        })
    }
}

#[test]
fn production_shaped_batch_refuses_duplicate_units_fields_unknown_controls_and_policy_skew() {
    let units = ["xochitl.service", "rm-sync.service"];
    let raw = String::from_utf8(bytes(&units)).unwrap();
    for bad in [
        raw.replace("Id=xochitl.service", "Id=foreign.service"),
        raw.replace("Nice=19", "Nice=19\nNice=19"),
        raw.replace("Nice=19", "Unknown=19"),
        raw.replace("Nice=19", "Nice=19\r"),
        raw.replace("\n\n", "\n"),
        format!("{raw}\n{raw}"),
    ] {
        assert!(ServiceBatch::parse(&units, bad.as_bytes()).is_err());
    }
    let skew = raw.replace("NotifyAccess=none", "NotifyAccess=all");
    let batch = ServiceBatch::parse(&units, skew.as_bytes()).unwrap();
    let expected = vendor::policy("rm-sync.service");
    assert!(vendor::check_policy(
        &batch
            .policy_bytes("rm-sync.service", expected.keys().copied())
            .unwrap(),
        &expected
    )
    .is_err());
}
#[test]
fn actual_batch_subprocess_cancellation_preserves_cause_and_reaps_child() {
    let mut polls = 0;
    let result = observe_many_with(
        &["xochitl.service"],
        Instant::now() + Duration::from_secs(3),
        &mut || {
            polls += 1;
            if polls >= 2 {
                return crate::workflow::resident_request::admission(
                    Instant::now() + Duration::from_secs(1),
                    &mut || Ok(crate::device::request_input::InputProgress::Canceled),
                    &mut || Ok(()),
                );
            }
            Ok(())
        },
        |_, deadline, pulse| {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "exec sleep 10"]);
            run_with_progress(command, deadline, pulse)
        },
    );
    let error = result.err().expect("cancel refused batch");
    let child = error
        .downcast_ref::<CommandFailure>()
        .expect("actual runner child receipt");
    assert!(child.reaped, "{child:?}");
    assert!(!std::path::Path::new(&format!("/proc/{}", child.pid)).exists());
    assert!(crate::workflow::resident_request::is_request_retirement(
        &error
    ));
}
