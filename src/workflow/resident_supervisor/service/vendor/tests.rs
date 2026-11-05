use super::*;
use std::cell::Cell;

fn sample(fields: &BTreeMap<&str, &str>) -> String {
    fields.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

#[test]
fn loaded_policy_rejects_schema_and_dependency_or_launch_changes() {
    for unit in [UNIT, "xochitl.service"] {
        let expected = policy(unit);
        let original = sample(&expected);
        check_policy(original.as_bytes(), &expected).unwrap();
        for key in expected.keys() {
            let mut fields = expected.clone();
            fields.remove(key);
            assert!(
                check_policy(sample(&fields).as_bytes(), &expected).is_err(),
                "missing {key}"
            );
            let duplicate = format!("{original}{key}={}\n", expected[key]);
            assert!(
                check_policy(duplicate.as_bytes(), &expected).is_err(),
                "duplicate {key}"
            );
            let mut fields = expected.clone();
            fields.insert(key, "unexpected");
            assert!(
                check_policy(sample(&fields).as_bytes(), &expected).is_err(),
                "changed {key}"
            );
        }
        for bad in [
            format!("{original}Unknown=\n"),
            original.replace('\n', "\r\n"),
            "x".repeat(LIMIT + 1),
        ] {
            assert!(check_policy(bad.as_bytes(), &expected).is_err());
        }
    }
}

#[test]
fn dependency_order_is_not_identity_but_duplicate_tokens_are_rejected() {
    let expected = policy(UNIT);
    let original = sample(&expected);
    let permuted = original
        .replace(
            "Requires=system.slice sysinit.target",
            "Requires=sysinit.target system.slice",
        )
        .replace("HUP INT PIPE TERM", "TERM PIPE HUP INT");
    check_policy(permuted.as_bytes(), &expected).unwrap();
    for bad in [
        original.replace(
            "BindsTo=xochitl.service",
            "BindsTo=xochitl.service xochitl.service",
        ),
        original.replace("BindsTo=xochitl.service", "BindsTo=other-xochitl.service"),
        original.replace("Nice=19", "Nice=019"),
        original.replace("KillSignal=15", "KillSignal=9"),
    ] {
        assert!(check_policy(bad.as_bytes(), &expected).is_err());
    }
}

#[test]
fn active_service_can_wait_for_name_without_recapturing_the_incarnation() {
    let acquisitions = Cell::new(0);
    let replies = Cell::new(0);
    let value = await_ready(
        Instant::now() + Duration::from_secs(1),
        &mut || Ok(()),
        || {
            acquisitions.set(acquisitions.get() + 1);
            Ok(Some(42))
        },
        |value| {
            assert_eq!(*value, 42);
            replies.set(replies.get() + 1);
            Ok(replies.get() > 1)
        },
    )
    .unwrap();
    assert_eq!(value, 42);
    assert_eq!(acquisitions.get(), 1);
    assert_eq!(replies.get(), 3); // missing, ready, delivery recheck
}

#[test]
fn process_replacement_while_waiting_for_name_is_not_recaptured() {
    let acquisitions = Cell::new(0);
    let replies = Cell::new(0);
    let error = await_ready(
        Instant::now() + Duration::from_secs(1),
        &mut || Ok(()),
        || {
            acquisitions.set(acquisitions.get() + 1);
            Ok(Some(42))
        },
        |_| {
            replies.set(replies.get() + 1);
            ensure!(replies.get() == 1, "retained process changed");
            Ok(false)
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("retained process changed"));
    assert_eq!(acquisitions.get(), 1);
}

#[test]
fn restoration_withholds_delivery_after_cancel_deadline_or_lost_readiness() {
    assert!(await_ready::<()>(
        Instant::now(),
        &mut || Ok(()),
        || panic!("expired"),
        |_| panic!("expired")
    )
    .is_err());
    let checks = Cell::new(0);
    assert!(await_ready(
        Instant::now() + Duration::from_secs(1),
        &mut || {
            checks.set(checks.get() + 1);
            ensure!(checks.get() == 1, "delivery canceled");
            Ok(())
        },
        || Ok(Some(1)),
        |_| Ok(true)
    )
    .is_err());
    let replies = Cell::new(0);
    assert!(await_ready(
        Instant::now() + Duration::from_secs(1),
        &mut || Ok(()),
        || Ok(Some(1)),
        |_| {
            replies.set(replies.get() + 1);
            Ok(replies.get() == 1)
        }
    )
    .is_err());
}
