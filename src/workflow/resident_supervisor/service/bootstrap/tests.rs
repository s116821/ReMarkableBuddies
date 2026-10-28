use super::*;
const NONCE: &str = "0123456789abcdef0123456789abcdef";

fn candidate(version: u32, kind: Kind, capture: bool, companion: bool) -> Candidate {
    Candidate {
        version,
        kind,
        nonce: NONCE.into(),
        fault: Fault::None,
        boot: String::new(),
        controller_sha256: "11".repeat(32),
        observer_sha256: "22".repeat(32),
        backup_sha256: "33".repeat(32),
        stock: ExpectedService {
            baseline: "original-stock".into(),
            executable_sha256: "44".repeat(32),
        },
        buddy: ExpectedService {
            baseline: "original-buddy".into(),
            executable_sha256: "55".repeat(32),
        },
        source: Vec::new(),
        unit_files: Vec::new(),
        capture: capture.then(|| setup::CapturePlan {
            document: String::new(),
            page: String::new(),
            order: Vec::new(),
            opening_metadata: None,
        }),
        companion: companion.then(|| "original-sync".into()),
    }
}

#[test]
fn native_companion_requires_explicit_candidate_version_and_capture() {
    for version in 1..=4 {
        for kind in [Kind::Native, Kind::Fixture] {
            for capture in [false, true] {
                for companion in [false, true] {
                    let c = candidate(version, kind, capture, companion);
                    let valid = matches!(
                        (version, kind, capture, companion),
                        (1, _, false, false)
                            | (2, Kind::Native, true, false)
                            | (3, Kind::Native, true, true)
                    );
                    assert_eq!(
                        c.validate_version().is_ok(),
                        valid,
                        "{version} {kind:?} {capture} {companion}"
                    );
                    let json = serde_json::to_value(&c).unwrap();
                    assert_eq!(json.get("companion").is_some(), companion);
                }
            }
        }
    }
}

#[test]
fn ownership_binds_the_exact_original_companion_and_version_to_candidate() {
    for c in [
        candidate(1, Kind::Fixture, false, false),
        candidate(2, Kind::Native, true, false),
        candidate(3, Kind::Native, true, true),
    ] {
        let record = Ownership {
            version: if c.companion.is_some() { 2 } else { 1 },
            candidate_sha256: "66".repeat(32),
            actor: "original-actor".into(),
            stock: c.stock.baseline.clone(),
            buddy: c.buddy.baseline.clone(),
            companion: c.companion.clone(),
        };
        record.validate(&c, &"66".repeat(32)).unwrap();
        assert!(record.validate(&c, &"77".repeat(32)).is_err());
        let bytes = serde_json::to_vec(&record).unwrap();
        for field in ["stock", "buddy", "companion"] {
            let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            changed[field] = "other-incarnation".into();
            assert!(serde_json::from_value::<Ownership>(changed)
                .unwrap()
                .validate(&c, &"66".repeat(32))
                .is_err());
        }
        let mut changed: Ownership = serde_json::from_slice(&bytes).unwrap();
        changed.version = 3 - changed.version;
        assert!(changed.validate(&c, &"66".repeat(32)).is_err());
    }
}

#[test]
fn recovery_retains_the_verified_pair_and_withholds_buddy_on_later_change() {
    use std::cell::Cell;
    let observed = Cell::new((41, 42));
    let captures = Cell::new(0);
    let result = recovery_sequence(
        || {
            captures.set(captures.get() + 1);
            Ok(observed.get())
        },
        |original| {
            assert_eq!(*original, (41, 42));
            observed.set((41, 43)); // companion replaced during source inspection
            Ok(())
        },
        |original| {
            ensure!(
                *original == observed.get(),
                "restored pair changed; Buddy withheld"
            );
            panic!("must not restore Buddy after replacement")
        },
    );
    assert!(result.unwrap_err().to_string().contains("Buddy withheld"));
    assert_eq!(captures.get(), 1);
}

#[test]
fn fixture_targets_and_native_units_cannot_be_substituted() {
    for role in ["stock", "buddy"] {
        assert_eq!(target(Kind::Fixture, NONCE, role), owned(NONCE, role));
        assert!(executable(Kind::Fixture, NONCE, role).starts_with(stage(NONCE)));
        assert!(unit_text(NONCE, Kind::Native, role).is_err());
    }
    assert_eq!(target(Kind::Native, NONCE, "stock"), "xochitl.service");
    assert_eq!(target(Kind::Native, NONCE, "buddy"), "reader-buddy.service");
    assert_eq!(source_root(Kind::Native, NONCE), Path::new(DATA));
    assert_eq!(
        source_root(Kind::Fixture, NONCE),
        stage(NONCE).join("fixture-data")
    );
    assert_eq!(argv(Kind::Native, NONCE, "stock"), [STOCK, "--system"]);
    assert_eq!(
        argv(Kind::Native, NONCE, "buddy"),
        ["/opt/bin/reader-buddy"]
    );
    assert_eq!(executable(Kind::Native, NONCE, "buddy"), Path::new(BUDDY));
}

#[test]
fn units_supply_separate_pid1_recovery_and_control_group_bounds() {
    for kind in [Kind::Native, Kind::Fixture] {
        let actor = unit_text(NONCE, kind, "actor").unwrap();
        let recovery = unit_text(NONCE, kind, "recovery").unwrap();
        assert!(actor.contains("Type=exec\n"));
        assert!(actor.contains("OnSuccessJobMode=fail\nOnFailureJobMode=fail\n"));
        assert!(actor.contains(&format!(
            "OnSuccess={}\nOnFailure={}\n",
            owned(NONCE, "recovery"),
            owned(NONCE, "recovery")
        )));
        assert!(recovery.contains(&format!(
            "Conflicts={}\nAfter={}\n",
            owned(NONCE, "actor"),
            owned(NONCE, "actor")
        )));
        assert!(recovery.contains("Type=oneshot\nTimeoutStartSec=130s\n"));
        for unit in [&actor, &recovery] {
            assert!(unit.contains("WatchdogSec=0\n"));
            assert!(unit.contains("KillMode=control-group\nSendSIGKILL=yes\nTimeoutStopSec=5s\n"));
            assert!(!unit.contains("DefaultDependencies=no"));
            assert!(!unit.contains("ExecStopPost="));
            assert!(unit.contains("LimitFSIZE=67108864\n")); // permits the bounded stock copy
        }
        assert!(actor.contains(if kind == Kind::Native {
            "RuntimeMaxSec=310s\n"
        } else {
            "RuntimeMaxSec=12s\n"
        }));
    }
}

#[test]
fn diagnostic_triggers_cannot_replace_conflicting_jobs() {
    let fields = BTreeMap::from([
        ("OnSuccessJobMode".into(), "fail".into()),
        ("OnFailureJobMode".into(), "fail".into()),
    ]);
    diagnostic_trigger_modes(&fields).unwrap();
    for key in ["OnSuccessJobMode", "OnFailureJobMode"] {
        for value in [
            "replace",
            "replace-irreversibly",
            "ignore-dependencies",
            "",
            "fail replace",
        ] {
            let mut changed = fields.clone();
            changed.insert(key.into(), value.into());
            assert!(diagnostic_trigger_modes(&changed).is_err());
        }
        let mut missing = fields.clone();
        missing.remove(key);
        assert!(diagnostic_trigger_modes(&missing).is_err());
    }
}

#[test]
fn watchdog_infinity_is_only_an_unstarted_helper_sentinel() {
    let mut s = Snapshot {
        fields: [
            ("WatchdogUSec", "infinity"),
            ("Type", "oneshot"),
            ("ActiveState", "inactive"),
            ("SubState", "dead"),
            ("MainPID", "0"),
            ("ControlPID", "0"),
            ("ExecMainPID", "0"),
            ("ExecMainStartTimestampMonotonic", "0"),
            ("InvocationID", ""),
            ("ControlGroup", ""),
            ("Job", ""),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect(),
    };
    assert!(diagnostic_watchdog_disabled(&s, true));
    assert!(!diagnostic_watchdog_disabled(&s, false));
    for (key, value) in [
        ("WatchdogUSec", "1s"),
        ("Type", "exec"),
        ("ActiveState", "activating"),
        ("SubState", "start"),
        ("MainPID", "42"),
        ("ControlPID", "42"),
        ("ExecMainPID", "42"),
        ("ExecMainStartTimestampMonotonic", "123"),
        ("InvocationID", "0123456789abcdef0123456789abcdef"),
        ("ControlGroup", "/system.slice/fixture.service"),
        ("Job", "42"),
    ] {
        let mut changed = s.clone();
        changed.fields.insert(key.into(), value.into());
        assert!(!diagnostic_watchdog_disabled(&changed, true), "{key}");
        changed.fields.remove(key);
        assert!(
            !diagnostic_watchdog_disabled(&changed, true),
            "missing {key}"
        );
    }
    s.fields.insert("WatchdogUSec".into(), "0".into());
    assert!(diagnostic_watchdog_disabled(&s, false));
}

#[test]
fn completed_actor_watchdog_requires_original_absence_then_stable_resampling() {
    use std::cell::Cell;
    let mut snapshot = Snapshot {
        fields: STATE
            .iter()
            .chain(POLICY)
            .map(|key| ((*key).into(), String::new()))
            .collect(),
    };
    snapshot.fields.extend(
        [
            ("WatchdogUSec", "infinity"),
            ("Type", "exec"),
            ("ActiveState", "inactive"),
            ("SubState", "dead"),
            ("MainPID", "0"),
            ("ControlPID", "0"),
            ("ExecMainPID", "0"),
            ("ExecMainStartTimestampMonotonic", "0"),
            ("InvocationID", ""),
            ("ControlGroup", ""),
            ("Job", ""),
            ("KillMode", "control-group"),
            ("After", "a.target b.service"),
            ("Conflicts", "c.target d.service"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into())),
    );
    let terminal = || RecoveryObservation {
        identity: TerminalIdentity::Cleared,
        group: GroupObservation::OriginalAbsent,
        policy: snapshot.policy(),
        watchdog: WatchdogObservation::ClearedUninitialized,
    };
    let proven = Cell::new(false);
    let mut reordered = snapshot.clone();
    reordered
        .fields
        .insert("After".into(), "b.service a.target".into());
    reordered
        .fields
        .insert("Conflicts".into(), "d.service c.target".into());
    assert_ne!(snapshot, reordered);
    assert!(cleared_actor_watchdog(&snapshot, || Ok(Some(terminal())), || Ok(reordered)).unwrap());
    assert!(cleared_actor_watchdog(
        &snapshot,
        || {
            proven.set(true);
            Ok(Some(terminal()))
        },
        || {
            assert!(proven.get(), "resampling must follow original-group proof");
            Ok(snapshot.clone())
        }
    )
    .unwrap());
    assert!(!cleared_actor_watchdog(&snapshot, || Ok(None), || panic!("not stopped")).unwrap());
    assert!(cleared_actor_watchdog(
        &snapshot,
        || anyhow::bail!("original group replaced"),
        || panic!("failed proof")
    )
    .is_err());
    for change in 0..5 {
        let mut observed = terminal();
        match change {
            0 => observed.identity = TerminalIdentity::Retained,
            1 => observed.group = GroupObservation::EmptyOriginal,
            2 => observed.group = GroupObservation::Populated,
            3 => observed.watchdog = WatchdogObservation::Runtime,
            _ => {
                let mut changed = snapshot.clone();
                changed.fields.insert("KillMode".into(), "process".into());
                observed.policy = changed.policy();
            }
        }
        assert!(!cleared_actor_watchdog(
            &snapshot,
            || Ok(Some(observed)),
            || panic!("invalid stopped observation")
        )
        .unwrap());
    }
    for (key, value) in [
        ("WatchdogUSec", "0"),
        ("ActiveState", "activating"),
        ("MainPID", "42"),
        ("InvocationID", "12"),
        ("Job", "42"),
        ("KillMode", "process"),
    ] {
        let mut changed = snapshot.clone();
        changed.fields.insert(key.into(), value.into());
        assert!(
            !cleared_actor_watchdog(&snapshot, || Ok(Some(terminal())), || Ok(changed)).unwrap(),
            "changed resample {key}"
        );
    }
    assert!(cleared_actor_watchdog(
        &snapshot,
        || Ok(Some(terminal())),
        || anyhow::bail!("resample unavailable")
    )
    .is_err());
    for (role, name) in [
        ("actor", "actor"),
        ("recovery", "recovery"),
        ("recovery", "actor"),
    ] {
        assert!(!diagnostic_actor_watchdog(&snapshot, role, name, None, Instant::now()).unwrap());
    }
    let mut partial = snapshot.clone();
    partial.fields.remove("ExecMainPID");
    assert!(!cleared_actor_watchdog(
        &partial,
        || panic!("incomplete clearing"),
        || panic!("incomplete clearing")
    )
    .unwrap());
}

#[test]
fn nonce_digest_and_candidate_schema_refuse_ambiguous_inputs() {
    nonce(NONCE).unwrap();
    for value in ["", "../escape", "0123456789abcdef0123456789abcdeF"] {
        assert!(nonce(value).is_err());
    }
    assert!(digest(&"00".repeat(32)).is_err());
    assert!(digest(&"AB".repeat(32)).is_err());
    assert_eq!(digest(&"12".repeat(32)).unwrap(), [0x12; 32]);
    let service = ExpectedService {
        baseline: "observed".into(),
        executable_sha256: "12".repeat(32),
    };
    let mut value = serde_json::to_value(&service).unwrap();
    value["unexpected"] = true.into();
    assert!(serde_json::from_value::<ExpectedService>(value).is_err());
    assert!(serde_json::from_str::<ExpectedService>(
        r#"{"baseline":"one","baseline":"two","executable_sha256":"x"}"#
    )
    .is_err());
}

#[test]
fn deadlines_and_record_names_cannot_be_extended_or_escape_owned_root() {
    assert!(check(Instant::now()).is_err());
    assert!(write_new(Path::new("/not-created"), "../outside", b"no").is_err());
    assert!(write_new(Path::new("/not-created"), "", b"no").is_err());
}

#[test]
fn diagnostic_expiry_cannot_prevent_stock_recovery_or_admit_buddy() {
    use std::cell::Cell;
    let restored = Cell::new(false);
    let result = recovery_sequence(
        || {
            restored.set(true);
            Ok(())
        },
        |_| {
            assert!(restored.get(), "inventory ran before stock recovery");
            check(Instant::now())
        },
        |_| panic!("expired preservation must withhold Buddy"),
    );
    assert!(result.is_err());
    assert!(restored.get());
}

#[test]
fn uncertain_stock_restoration_withholds_diagnostics_and_buddy() {
    let result = recovery_sequence::<()>(
        || anyhow::bail!("uncertain original group"),
        |_| panic!("stock restoration was not verified"),
        |_| panic!("Buddy must remain stopped"),
    );
    assert!(result.unwrap_err().to_string().contains("uncertain"));
}

#[test]
fn handoff_cancellation_or_integrity_loss_never_submits_stop() {
    use crate::device::interaction::Interaction;
    for poll in [
        Ok(vec![Interaction::Invalidated]),
        Err(anyhow::anyhow!("lost old input")),
    ] {
        let mut input = Some(RequestInput::scripted(vec![poll]));
        assert!(stop_with_input(
            &mut input,
            || panic!("no stop after canceled or lost admission"),
            || panic!("not submitted"),
            true,
            || panic!("no replacement of invalid admission"),
            Instant::now() + Duration::from_secs(1),
        )
        .is_err());
    }
}

#[test]
fn handoff_ends_old_admission_and_requires_terminal_proof_before_fresh_input() {
    use std::cell::Cell;
    let submitted = Cell::new(0);
    let samples = Cell::new(0);
    let created = Cell::new(0);
    // The old admission has no further polls: it ends after successful submission.
    let mut input = Some(RequestInput::scripted(vec![Ok(vec![])]));
    stop_with_input(
        &mut input,
        || {
            submitted.set(submitted.get() + 1);
            Ok(())
        },
        || {
            assert_eq!(submitted.get(), 1);
            assert_eq!(created.get(), 0);
            samples.set(samples.get() + 1);
            Ok(samples.get() == 2)
        },
        true,
        || {
            assert_eq!(samples.get(), 2);
            created.set(created.get() + 1);
            Ok(RequestInput::scripted(vec![
                Ok(vec![]),
                Err(anyhow::anyhow!("later inventory loss")),
            ]))
        },
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!((submitted.get(), created.get()), (1, 1));
    // The fresh guard must remain in force before the subsequent stock stop.
    assert!(stop_with_input(
        &mut input,
        || panic!("lost fresh guard cannot stop stock"),
        || panic!("not submitted"),
        false,
        || panic!("cannot replace fresh guard"),
        Instant::now() + Duration::from_secs(1),
    )
    .is_err());
}

#[test]
fn handoff_failure_or_population_expiry_never_creates_new_admission_or_retries() {
    use std::cell::Cell;
    for fault in ["submit", "terminal", "populated", "deadline"] {
        let submitted = Cell::new(0);
        let mut input = Some(RequestInput::scripted(vec![Ok(vec![])]));
        let deadline = if fault == "deadline" {
            Instant::now()
        } else {
            Instant::now() + Duration::from_millis(10)
        };
        assert!(stop_with_input(
            &mut input,
            || {
                submitted.set(submitted.get() + 1);
                ensure!(fault != "submit", "submission failed");
                Ok(())
            },
            || {
                ensure!(fault != "terminal", "identity changed");
                Ok(false)
            },
            true,
            || panic!("no new admission without terminal proof within deadline"),
            deadline,
        )
        .is_err());
        assert_eq!(submitted.get(), usize::from(fault != "deadline"));
    }
}

#[test]
fn fresh_contact_or_integrity_failure_refuses_after_handoff_without_reset() {
    use crate::device::interaction::Interaction;
    for poll in [
        Ok(vec![Interaction::Invalidated]),
        Err(anyhow::anyhow!("fresh inventory changed")),
    ] {
        let mut input = Some(RequestInput::scripted(vec![Ok(vec![])]));
        assert!(stop_with_input(
            &mut input,
            || Ok(()),
            || Ok(true),
            true,
            || Ok(RequestInput::scripted(vec![poll])),
            Instant::now() + Duration::from_secs(1),
        )
        .is_err());
        assert!(input.is_some(), "failed fresh admission is not cleared");
    }
}

#[test]
fn stock_shutdown_keeps_the_existing_observation_guard() {
    let mut input = Some(RequestInput::scripted(vec![
        Ok(vec![]),
        Err(anyhow::anyhow!("inventory changed during stock stop")),
    ]));
    assert!(stop_with_input(
        &mut input,
        || Ok(()),
        || panic!("lost input cannot advance to terminal proof"),
        false,
        || panic!("stock stop cannot create a new admission"),
        Instant::now() + Duration::from_secs(1),
    )
    .is_err());
}
