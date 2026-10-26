use super::*;

#[test]
fn final_handoff_refusal_or_expiry_never_calls_stop() {
    for expired in [false, true] {
        let mut calls = 0;
        let deadline = Instant::now() + Duration::from_millis(20);
        let result = checked_handoff(
            deadline,
            || {
                if !expired {
                    anyhow::bail!("canceled at stop boundary");
                }
                thread::sleep(
                    deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
                );
                Ok(())
            },
            || {
                calls += 1;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 0);
    }
    let mut steps = Vec::new();
    let steps = std::cell::RefCell::new(&mut steps);
    checked_handoff(
        Instant::now() + Duration::from_secs(1),
        || {
            steps.borrow_mut().push("admission");
            Ok(())
        },
        || {
            steps.borrow_mut().push("stop");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(**steps.borrow(), ["admission", "stop"]);
}

fn snapshot(
    unit: &str,
    policy: &BTreeMap<String, String>,
    active: &str,
    sub: &str,
    pid: u32,
) -> service::Snapshot {
    let mut fields: BTreeMap<String, String> = service::STATE
        .iter()
        .map(|key| ((*key).into(), String::new()))
        .collect();
    fields.extend(policy.clone());
    for (key, value) in [
        ("Id", unit),
        ("LoadState", "loaded"),
        ("ActiveState", active),
        ("SubState", sub),
        ("ControlPID", "0"),
        ("Job", ""),
        ("InvocationID", "12121212121212121212121212121212"),
        ("ExecMainStartTimestampMonotonic", "1234"),
    ] {
        fields.insert(key.into(), value.into());
    }
    fields.insert("MainPID".into(), pid.to_string());
    fields.insert("ExecMainPID".into(), pid.to_string());
    fields.insert("ControlGroup".into(), format!("/system.slice/{unit}"));
    parse(fields, unit)
}
fn parse(fields: BTreeMap<String, String>, unit: &str) -> service::Snapshot {
    let bytes = fields
        .into_iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect::<String>();
    service::Snapshot::parse(bytes.as_bytes(), unit).unwrap()
}
fn changed(s: &service::Snapshot, changes: &[(&str, &str)]) -> service::Snapshot {
    let mut fields: BTreeMap<String, String> = service::STATE
        .iter()
        .chain(service::POLICY)
        .map(|key| ((*key).into(), s.property(key).unwrap().into()))
        .collect();
    for (key, value) in changes {
        fields.insert((*key).into(), (*value).into());
    }
    let unit = fields["Id"].clone();
    parse(fields, &unit)
}

#[test]
fn installed_admission_requires_current_buddy_and_idle_helper() {
    let f = crate::workflow::resident_supervisor::tests::Fixture::new();
    let record = f.record();
    // Pure service predicate fixture; this does not model kernel membership.
    let mut group = record.group().clone();
    group.unit = BUDDY.into();
    group.mount_path = MOUNT.into();
    let profile = super::super::tests::profile(&record);
    let buddy = snapshot(
        BUDDY,
        &profile.buddy_policy,
        "active",
        "running",
        std::process::id(),
    );
    buddy_state(&buddy, &group, &profile).unwrap();
    for (key, value) in [
        ("ActiveState", "activating"),
        ("ControlPID", "42"),
        ("Job", "42"),
        ("InvocationID", ""),
        ("MainPID", "42"),
        ("ExecMainPID", "42"),
        ("ExecMainStartTimestampMonotonic", "0"),
        ("ControlGroup", "/wrong"),
        ("Restart", "always"),
        ("WatchdogUSec", "0"),
        ("NeedDaemonReload", "yes"),
    ] {
        assert!(
            buddy_state(&changed(&buddy, &[(key, value)]), &group, &profile).is_err(),
            "{key}"
        );
    }
    let helper = snapshot(HELPER, &profile.helper_policy, "inactive", "dead", 0);
    idle_helper(&helper, &profile).unwrap();
    for (key, value) in [
        ("ActiveState", "failed"),
        ("Job", "42"),
        ("MainPID", "42"),
        ("ControlPID", "42"),
        ("Type", "simple"),
        ("TimeoutStartUSec", "1min"),
    ] {
        assert!(
            idle_helper(&changed(&helper, &[(key, value)]), &profile).is_err(),
            "{key}"
        );
    }
}

#[test]
fn first_activation_accepts_only_the_unstarted_helper_watchdog_sentinel() {
    let f = crate::workflow::resident_supervisor::tests::Fixture::new();
    let profile = super::super::tests::profile(&f.record());
    let used = snapshot(HELPER, &profile.helper_policy, "inactive", "dead", 0);
    idle_helper(&used, &profile).unwrap();
    let fresh = changed(
        &used,
        &[
            ("WatchdogUSec", "infinity"),
            ("InvocationID", ""),
            ("ExecMainStartTimestampMonotonic", "0"),
            ("ControlGroup", ""),
        ],
    );
    idle_helper(&fresh, &profile).unwrap();
    for (key, value) in [
        ("WatchdogUSec", "1s"),
        ("ActiveState", "activating"),
        ("SubState", "start"),
        ("MainPID", "42"),
        ("ControlPID", "42"),
        ("ExecMainPID", "42"),
        ("ExecMainStartTimestampMonotonic", "1234"),
        ("InvocationID", "12121212121212121212121212121212"),
        (
            "ControlGroup",
            "/system.slice/reader-buddy-stock-recovery.service",
        ),
        ("Job", "42"),
        ("Type", "simple"),
        ("DropInPaths", "/tmp/override.conf"),
        ("NeedDaemonReload", "yes"),
        ("Restart", "always"),
    ] {
        assert!(
            idle_helper(&changed(&fresh, &[(key, value)]), &profile).is_err(),
            "{key}"
        );
    }
    // The special case must not normalize ordinary or active recovery policy.
    assert!(match_policy(&fresh.policy(), &profile.helper_policy).is_err());
    assert!(idle_helper(&changed(&used, &[("WatchdogUSec", "infinity")]), &profile).is_err());
    assert_eq!(profile.helper_policy["WatchdogUSec"], "0");
}

#[test]
fn installed_cleared_watchdog_policy_requires_actual_group_absence() {
    let f = crate::workflow::resident_supervisor::tests::Fixture::new();
    let profile = super::super::tests::profile(&f.record());
    for (unit, policy) in [
        (BUDDY, &profile.buddy_policy),
        (STOCK, &profile.stock_policy),
    ] {
        let mut expected = policy.clone();
        // Both a disabled interval and the Buddy's finite watchdog are covered.
        if unit == STOCK {
            expected.insert("WatchdogUSec".into(), "0".into());
        }
        let original = snapshot(unit, &expected, "active", "running", 42);
        let cleared = changed(
            &original,
            &[
                ("ActiveState", "inactive"),
                ("SubState", "dead"),
                ("MainPID", "0"),
                ("ExecMainPID", "0"),
                ("ExecMainStartTimestampMonotonic", "0"),
                ("InvocationID", ""),
                ("ControlGroup", ""),
                ("WatchdogUSec", "infinity"),
            ],
        );
        assert!(match_policy(&cleared.policy(), &expected).is_err());
        match_observed_policy(
            &cleared.policy(),
            &expected,
            cleared.watchdog_observation(GroupObservation::OriginalAbsent),
        )
        .unwrap();
        for group in [GroupObservation::Populated, GroupObservation::EmptyOriginal] {
            assert!(match_observed_policy(
                &cleared.policy(),
                &expected,
                cleared.watchdog_observation(group)
            )
            .is_err());
        }
        let wrong = changed(&cleared, &[("KillMode", "process")]);
        assert!(match_observed_policy(
            &wrong.policy(),
            &expected,
            wrong.watchdog_observation(GroupObservation::OriginalAbsent)
        )
        .is_err());
        // Neither normal policy matching nor the expected profile is normalized.
        assert_ne!(expected["WatchdogUSec"], "infinity");
        assert_eq!(cleared.property("WatchdogUSec"), Some("infinity"));
    }
}

#[test]
fn stopped_original_requires_stable_identity_terminal_state_and_empty_group() {
    let f = crate::workflow::resident_supervisor::tests::Fixture::new();
    let profile = super::super::tests::profile(&f.record());
    let original = snapshot(STOCK, &profile.stock_policy, "active", "running", 42);
    assert!(!stopped_stock(&original, &original, GroupObservation::Populated).unwrap());
    let stopping = changed(
        &original,
        &[
            ("ActiveState", "deactivating"),
            ("SubState", "stop-sigterm"),
            ("Job", "42"),
            ("ControlPID", "43"),
        ],
    );
    assert!(!stopped_stock(&stopping, &original, GroupObservation::Populated).unwrap());
    let stopped = changed(
        &original,
        &[
            ("ActiveState", "inactive"),
            ("SubState", "dead"),
            ("MainPID", "0"),
        ],
    );
    assert!(stopped_stock(&stopped, &original, GroupObservation::Populated).is_err());
    for group in [
        GroupObservation::EmptyOriginal,
        GroupObservation::OriginalAbsent,
    ] {
        assert!(stopped_stock(&stopped, &original, group).unwrap());
    }
    for (key, value) in [
        ("InvocationID", ""),
        ("ExecMainPID", "0"),
        ("ExecMainStartTimestampMonotonic", "0"),
        ("Job", "42"),
        ("ControlPID", "43"),
        ("MainPID", "44"),
        ("ControlGroup", "/wrong"),
        ("ActiveState", "activating"),
    ] {
        assert!(
            stopped_stock(
                &changed(&stopped, &[(key, value)]),
                &original,
                GroupObservation::OriginalAbsent
            )
            .is_err(),
            "{key}"
        );
    }
    let cleared = changed(
        &stopped,
        &[
            ("InvocationID", ""),
            ("ExecMainPID", "0"),
            ("ExecMainStartTimestampMonotonic", "0"),
            ("ControlGroup", ""),
        ],
    );
    assert!(stopped_stock(&cleared, &original, GroupObservation::OriginalAbsent).unwrap());
    assert!(stopped_stock(&cleared, &original, GroupObservation::EmptyOriginal).is_err());
    let failed = changed(
        &cleared,
        &[("ActiveState", "failed"), ("SubState", "failed")],
    );
    assert!(stopped_stock(&failed, &original, GroupObservation::OriginalAbsent).is_err());
}
