use super::*;
use crate::workflow::resident_supervisor::tests::Fixture;

fn baseline(f: &Fixture) -> ServiceBaseline {
    let record = f.record();
    let mut fields: BTreeMap<String, String> = STATE
        .iter()
        .chain(POLICY)
        .map(|key| ((*key).into(), String::new()))
        .collect();
    for (key, value) in [
        ("Id", "fixture.service"),
        ("LoadState", "loaded"),
        ("ActiveState", "active"),
        ("SubState", "running"),
        ("ControlPID", "0"),
        ("Job", ""),
        ("ControlGroup", "/system.slice/fixture.service"),
        ("KillMode", "control-group"),
        ("Delegate", "no"),
        ("Restart", "no"),
        ("NeedDaemonReload", "no"),
        ("SendSIGKILL", "yes"),
    ] {
        fields.insert(key.into(), value.into());
    }
    fields.insert("MainPID".into(), record.group().parent().pid.to_string());
    fields.insert(
        "ExecMainPID".into(),
        record.group().parent().pid.to_string(),
    );
    fields.insert("InvocationID".into(), "23".repeat(16));
    fields.insert("ExecMainStartTimestampMonotonic".into(), "1234".into());
    let original = Snapshot::parse(&encode_fields(&fields), "fixture.service").unwrap();
    ServiceBaseline {
        original,
        group: record.group().clone(),
    }
}

fn encode_fields(fields: &BTreeMap<String, String>) -> Vec<u8> {
    fields
        .iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect::<String>()
        .into_bytes()
}

fn changed(s: &Snapshot, fields: &[(&str, &str)]) -> Snapshot {
    let mut s = s.clone();
    for (key, value) in fields {
        s.fields.insert((*key).into(), (*value).into());
    }
    Snapshot::parse(&encode_fields(&s.fields), s.property("Id").unwrap()).unwrap()
}

fn terminal(s: &Snapshot) -> Snapshot {
    changed(
        s,
        &[
            ("ActiveState", "inactive"),
            ("SubState", "dead"),
            ("MainPID", "0"),
        ],
    )
}

fn clear(s: &Snapshot) -> Snapshot {
    changed(
        s,
        &[
            ("InvocationID", ""),
            ("ExecMainPID", "0"),
            ("ExecMainStartTimestampMonotonic", "0"),
            ("ControlGroup", ""),
        ],
    )
}

#[test]
fn selected_record_preserves_bytes_and_rejects_every_other_property_difference() {
    let f = Fixture::new();
    let mut selected = baseline(&f);
    selected.original = changed(
        &selected.original,
        &[
            ("After", "a.target b.target"),
            ("Conflicts", "c.target d.target"),
        ],
    );
    let bytes = selected.encode().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let mut fresh = ServiceBaseline::decode(&bytes).unwrap();
    fresh.original = changed(
        &fresh.original,
        &[
            ("After", "b.target a.target"),
            ("Conflicts", "d.target c.target"),
        ],
    );
    assert_ne!(fresh.encode().unwrap(), bytes);
    assert_eq!(fresh.selected_record(&text).unwrap(), text);
    assert_eq!(selected.encode().unwrap(), bytes);
    for key in STATE.iter().chain(POLICY) {
        let mut altered = ServiceBaseline::decode(&fresh.encode().unwrap()).unwrap();
        let value = format!("{} changed", altered.original.property(key).unwrap());
        let mut fields = altered.original.fields.clone();
        fields.insert((*key).into(), value);
        if let Ok(parsed) = Snapshot::parse(&encode_fields(&fields), "fixture.service") {
            altered.original = parsed;
            assert!(altered.selected_record(&text).is_err(), "changed {key}");
        }
        let mut missing = ServiceBaseline::decode(&bytes).unwrap();
        let mut fields = missing.original.fields.clone();
        fields.remove(*key);
        if let Ok(parsed) = Snapshot::parse(&encode_fields(&fields), "fixture.service") {
            missing.original = parsed;
            assert!(missing.selected_record(&text).is_err(), "missing {key}");
        }
    }
    for key in ["After", "Conflicts"] {
        let mut duplicate = ServiceBaseline::decode(&bytes).unwrap();
        duplicate
            .original
            .fields
            .insert(key.into(), "a.target a.target".into());
        let duplicate_text = String::from_utf8(duplicate.encode().unwrap()).unwrap();
        assert!(duplicate.selected_record(&duplicate_text).is_err());
        assert!(fresh.selected_record(&duplicate_text).is_err());
    }
}

#[test]
fn selected_record_requires_exact_group_process_and_context_identity() {
    let f = Fixture::new();
    let selected = baseline(&f);
    let bytes = selected.encode().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for pointer in [
        "/group/mount_path",
        "/group/unit",
        "/group/parent/pid",
        "/group/parent/start_ticks",
        "/group/context/boot",
        "/group/context/mount_record",
    ]
    .into_iter()
    .map(String::from)
    .chain(
        [
            "/group/group",
            "/group/context/mount_namespace",
            "/group/context/cgroup_namespace",
            "/group/context/mount",
            "/group/context/slice",
        ]
        .into_iter()
        .flat_map(|base| {
            ["device", "inode", "uid", "mode"]
                .into_iter()
                .map(move |field| format!("{base}/{field}"))
        }),
    ) {
        let mut altered = record.clone();
        let value = altered.pointer_mut(&pointer).unwrap();
        *value = if let Some(n) = value.as_u64() {
            serde_json::json!(n + 1)
        } else {
            serde_json::json!(format!("{}changed", value.as_str().unwrap()))
        };
        if let Ok(altered) = ServiceBaseline::decode(&serde_json::to_vec(&altered).unwrap()) {
            assert!(altered.selected_record(&text).is_err(), "changed {pointer}");
        }
    }
}

#[test]
fn cleared_stop_accepts_only_reordered_dependencies_with_original_group_absence() {
    let f = Fixture::new();
    let mut selected = baseline(&f);
    selected.original = changed(
        &selected.original,
        &[
            ("WatchdogUSec", "0"),
            (
                "After",
                "system.slice sysinit.target systemd-journald.socket basic.target",
            ),
            ("Conflicts", "shutdown.target fixture.target"),
        ],
    );
    let encoded = selected.encode().unwrap();
    let original = &selected.original;
    let cleared = changed(
        &clear(&terminal(original)),
        &[
            ("WatchdogUSec", "infinity"),
            (
                "After",
                "systemd-journald.socket basic.target sysinit.target system.slice",
            ),
            ("Conflicts", "fixture.target shutdown.target"),
        ],
    );
    let result = stopped_bracket(
        original,
        &cleared,
        GroupObservation::OriginalAbsent,
        &cleared,
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.identity, TerminalIdentity::Cleared);
    assert_eq!(result.watchdog, WatchdogObservation::ClearedUninitialized);
    assert_eq!(result.policy.property("After"), cleared.property("After"));
    assert_eq!(
        ServiceBaseline::decode(&encoded).unwrap().encode().unwrap(),
        encoded
    );
    assert_eq!(selected.encode().unwrap(), encoded);
    for group in [GroupObservation::EmptyOriginal, GroupObservation::Populated] {
        assert!(stopped_snapshot(&cleared, original, group).is_err());
    }
    for (key, value) in [
        ("After", "system.slice sysinit.target basic.target"),
        (
            "After",
            "system.slice system.slice sysinit.target systemd-journald.socket basic.target",
        ),
        ("Conflicts", "fixture.target shutdown.target extra.target"),
        (
            "Conflicts",
            "fixture.target shutdown.target shutdown.target",
        ),
        ("KillMode", "process"),
    ] {
        assert!(
            stopped_snapshot(
                &changed(&cleared, &[(key, value)]),
                original,
                GroupObservation::OriginalAbsent
            )
            .is_err(),
            "{key}"
        );
    }
    let reordered_between_samples =
        changed(&cleared, &[("After", original.property("After").unwrap())]);
    assert_ne!(cleared, reordered_between_samples);
    assert!(stopped_bracket(
        original,
        &cleared,
        GroupObservation::OriginalAbsent,
        &reordered_between_samples,
        Instant::now() + Duration::from_secs(1)
    )
    .unwrap()
    .is_some());
}

#[test]
fn cleared_watchdog_sentinel_requires_deleted_group_and_preserves_raw_policy() {
    let f = Fixture::new();
    for (kind, interval) in [("exec", "0"), ("notify", "2min")] {
        let original = changed(
            &baseline(&f).original,
            &[("Type", kind), ("WatchdogUSec", interval)],
        );
        let cleared = changed(
            &clear(&terminal(&original)),
            &[("WatchdogUSec", "infinity")],
        );
        let result = stopped_bracket(
            &original,
            &cleared,
            GroupObservation::OriginalAbsent,
            &cleared,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.identity, TerminalIdentity::Cleared);
        assert_eq!(result.watchdog, WatchdogObservation::ClearedUninitialized);
        assert_eq!(result.policy.property("WatchdogUSec"), Some("infinity"));
        assert_eq!(original.property("WatchdogUSec"), Some(interval));
        for group in [GroupObservation::EmptyOriginal, GroupObservation::Populated] {
            assert!(stopped_snapshot(&cleared, &original, group).is_err());
        }
        let retained = changed(&terminal(&original), &[("WatchdogUSec", "infinity")]);
        assert!(stopped_snapshot(&retained, &original, GroupObservation::OriginalAbsent).is_err());
        for (key, value) in [
            ("WatchdogUSec", "1s"),
            ("ActiveState", "deactivating"),
            ("SubState", "stop-sigterm"),
            ("MainPID", "42"),
            ("ControlPID", "42"),
            ("ExecMainPID", "42"),
            ("ExecMainStartTimestampMonotonic", "1234"),
            ("InvocationID", "34343434343434343434343434343434"),
            ("ControlGroup", "/system.slice/fixture.service"),
            ("Job", "42"),
            ("Type", "oneshot"),
            ("Restart", "always"),
            ("NeedDaemonReload", "yes"),
        ] {
            assert!(
                stopped_snapshot(
                    &changed(&cleared, &[(key, value)]),
                    &original,
                    GroupObservation::OriginalAbsent
                )
                .is_err(),
                "{kind}: {key}"
            );
        }
        let changed_interval = changed(&cleared, &[("WatchdogUSec", interval)]);
        assert!(stopped_bracket(
            &original,
            &cleared,
            GroupObservation::OriginalAbsent,
            &changed_interval,
            Instant::now() + Duration::from_secs(1)
        )
        .unwrap()
        .is_none());
    }
}

#[test]
fn records_round_trip_but_do_not_assert_live_kernel_ownership() {
    let f = Fixture::new();
    let baseline = baseline(&f);
    let bytes = baseline.encode().unwrap();
    let decoded = ServiceBaseline::decode(&bytes).unwrap();
    assert_eq!(decoded.encode().unwrap(), bytes);
    // Fixture files are deliberately not cgroups: current/stopped must NOT
    // convert deserialization into a live ownership result.
    assert!(decoded.group().observe().is_err());
    assert!(decoded.current(Instant::now()).is_err());
    assert!(decoded.stopped(Instant::now()).is_err());
}

#[test]
fn malformed_or_fabricated_record_relationships_refuse() {
    let f = Fixture::new();
    let baseline = baseline(&f);
    let bytes = baseline.encode().unwrap();
    assert!(ServiceBaseline::decode(&vec![b' '; RECORD_LIMIT + 1]).is_err());
    assert!(ServiceBaseline::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for key in ["version", "snapshot", "group"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(ServiceBaseline::decode(&serde_json::to_vec(&missing).unwrap()).is_err());
    }
    value["unexpected"] = true.into();
    assert!(ServiceBaseline::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let duplicate = format!(
        "{{\"version\":1,{}",
        std::str::from_utf8(&bytes).unwrap().trim_start_matches('{')
    );
    assert!(ServiceBaseline::decode(duplicate.as_bytes()).is_err());
    for (key, value) in [
        ("Id", "other.service"),
        ("MainPID", "0"),
        ("ExecMainPID", "0"),
        ("InvocationID", ""),
        ("ExecMainStartTimestampMonotonic", "0"),
        ("ActiveState", "inactive"),
        ("SubState", "dead"),
        ("LoadState", "not-found"),
        ("Job", "42"),
        ("ControlPID", "42"),
        ("ControlGroup", "/wrong"),
    ] {
        let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let s = changed(&baseline.original, &[(key, value)]);
        record["snapshot"] = String::from_utf8(encode_fields(&s.fields)).unwrap().into();
        assert!(
            ServiceBaseline::decode(&serde_json::to_vec(&record).unwrap()).is_err(),
            "{key}"
        );
    }
    for snapshot in [
        "Id=fixture.service\n".to_string(),
        format!(
            "{}MainPID=0\n",
            std::str::from_utf8(&encode_fields(&baseline.original.fields)).unwrap()
        ),
    ] {
        let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        record["snapshot"] = snapshot.into();
        assert!(ServiceBaseline::decode(&serde_json::to_vec(&record).unwrap()).is_err());
    }
}

#[test]
fn typed_terminal_bracket_requires_original_identity_group_and_policy() {
    let f = Fixture::new();
    let baseline = baseline(&f);
    let original = baseline.original();
    let stopped = terminal(original);
    let cleared = clear(&stopped);
    let deadline = Instant::now() + Duration::from_secs(5);
    for (s, group, identity) in [
        (
            &stopped,
            GroupObservation::EmptyOriginal,
            TerminalIdentity::Retained,
        ),
        (
            &stopped,
            GroupObservation::OriginalAbsent,
            TerminalIdentity::Retained,
        ),
        (
            &cleared,
            GroupObservation::OriginalAbsent,
            TerminalIdentity::Cleared,
        ),
    ] {
        let observed = stopped_bracket(original, s, group, s, deadline)
            .unwrap()
            .unwrap();
        assert_eq!(observed.identity, identity);
        assert_eq!(observed.group, group);
        assert_eq!(observed.policy, original.policy());
    }
    assert!(stopped_bracket(
        original,
        &stopped,
        GroupObservation::Populated,
        &stopped,
        deadline
    )
    .is_err());
    assert!(stopped_bracket(
        original,
        &cleared,
        GroupObservation::EmptyOriginal,
        &cleared,
        deadline
    )
    .is_err());
    for (key, value) in [
        ("InvocationID", ""),
        ("ExecMainPID", "0"),
        ("ExecMainStartTimestampMonotonic", "0"),
        ("Job", "42"),
        ("ControlPID", "42"),
        ("MainPID", "42"),
        ("KillMode", "process"),
        ("NeedDaemonReload", "yes"),
        ("ControlGroup", "/wrong"),
    ] {
        let bad = changed(&stopped, &[(key, value)]);
        for (a, b) in [(&bad, &stopped), (&stopped, &bad)] {
            assert!(
                stopped_bracket(original, a, GroupObservation::OriginalAbsent, b, deadline)
                    .is_err(),
                "{key}"
            );
        }
    }
    assert!(stopped_bracket(
        original,
        &stopped,
        GroupObservation::OriginalAbsent,
        &stopped,
        Instant::now()
    )
    .is_err());
}

#[test]
fn transitions_remain_pending_and_never_renew_the_deadline() {
    let f = Fixture::new();
    let baseline = baseline(&f);
    let original = baseline.original();
    let stopping = changed(
        original,
        &[
            ("ActiveState", "deactivating"),
            ("SubState", "stop-sigterm"),
            ("Job", "42"),
        ],
    );
    let stopped = terminal(original);
    let cleared = clear(&stopped);
    let deadline = Instant::now() + Duration::from_secs(5);
    for (a, b, group) in [
        (original, &stopping, GroupObservation::Populated),
        (&stopping, &stopped, GroupObservation::EmptyOriginal),
        (&stopped, &cleared, GroupObservation::OriginalAbsent),
    ] {
        assert!(stopped_bracket(original, a, group, b, deadline)
            .unwrap()
            .is_none());
    }
    assert!(stopped_bracket(
        original,
        &stopping,
        GroupObservation::Populated,
        &stopping,
        Instant::now()
    )
    .is_err());
}
