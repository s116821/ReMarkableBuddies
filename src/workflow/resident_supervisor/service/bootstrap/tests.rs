use super::*;
const NONCE: &str = "0123456789abcdef0123456789abcdef";

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
    assert_eq!(argv(Kind::Native, NONCE, "buddy"), [BUDDY]);
}

#[test]
fn units_supply_separate_pid1_recovery_and_control_group_bounds() {
    for kind in [Kind::Native, Kind::Fixture] {
        let actor = unit_text(NONCE, kind, "actor").unwrap();
        let recovery = unit_text(NONCE, kind, "recovery").unwrap();
        assert!(actor.contains("Type=exec\n"));
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
        || {
            assert!(restored.get(), "inventory ran before stock recovery");
            check(Instant::now())
        },
        || panic!("expired preservation must withhold Buddy"),
    );
    assert!(result.is_err());
    assert!(restored.get());
}

#[test]
fn uncertain_stock_restoration_withholds_diagnostics_and_buddy() {
    let result = recovery_sequence(
        || anyhow::bail!("uncertain original group"),
        || panic!("stock restoration was not verified"),
        || panic!("Buddy must remain stopped"),
    );
    assert!(result.unwrap_err().to_string().contains("uncertain"));
}
