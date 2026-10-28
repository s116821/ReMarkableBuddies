use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

fn end() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
fn binding() -> Binding {
    Binding {
        nonce: "0123456789abcdef0123456789abcdef".into(),
        candidate_sha256: "11".repeat(32),
        observer: ProcessIdentity {
            pid: 123,
            start_ticks: 10,
        },
        runtime: ProcessIdentity {
            pid: 124,
            start_ticks: 11,
        },
        boot: "11111111-1111-4111-8111-111111111111".into(),
        activation: "22222222-2222-4222-8222-222222222222".into(),
        profile_sha256: "22".repeat(32),
        executable_sha256: "33".repeat(32),
        adapter_sha256: "44".repeat(32),
    }
}
fn root() -> PathBuf {
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "run explicitly in a root-owned test container"
    );
    let root = PathBuf::from(format!("/root/capture-setup-test-{}", uuid::Uuid::new_v4()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    root
}
#[test]
fn capture_plan_validates_full_order_and_canonical_ids() {
    let mut plan = CapturePlan {
        opening_metadata: None,
        document: binding().boot,
        page: binding().activation,
        order: vec![binding().activation],
    };
    assert!(plan.facts().is_ok());
    plan.order.push(plan.page.clone());
    assert!(plan.facts().is_err());
    plan.order.clear();
    assert!(plan.facts().is_err());
    plan.order.push(binding().boot);
    assert!(plan.facts().is_err());
    plan.order = vec![plan.page.clone(); 18];
    assert!(plan.facts().is_err());
    plan.document = "../document".into();
    assert!(plan.facts().is_err());
}
#[test]
fn binding_rejects_ambiguous_or_incomplete_identity() {
    let b = binding();
    b.validate().unwrap();
    let json = serde_json::to_string(&b).unwrap();
    for bad in [
        json.replacen("{", "{\"unexpected\":true,", 1),
        json.replacen("{", &format!("{{\"nonce\":\"{}\",", b.nonce), 1),
    ] {
        assert!(serde_json::from_str::<Binding>(&bad).is_err());
    }
    let mut changed = b.clone();
    changed.runtime = changed.observer.clone();
    assert!(changed.validate().is_err());
    changed = b.clone();
    changed.candidate_sha256 = "00".repeat(32);
    assert!(changed.validate().is_err());
    changed = b;
    changed.runtime.start_ticks = 0;
    assert!(changed.validate().is_err());
}

struct Guard {
    canceled: Arc<AtomicBool>,
    drops: Arc<AtomicUsize>,
}
impl InputGuard for Guard {
    fn quiet(&mut self) -> Result<()> {
        ensure!(!self.canceled.load(Ordering::SeqCst), "test input canceled");
        Ok(())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
fn guard(drops: &Arc<AtomicUsize>) -> Guard {
    Guard {
        canceled: Arc::new(AtomicBool::new(false)),
        drops: drops.clone(),
    }
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn two_participant_setup_ends_both_guards_before_ready_and_acquires_once() {
    let root = root();
    let b = binding();
    let deadline = end();
    let drops = Arc::new(AtomicUsize::new(0));
    let acquisitions = Arc::new(AtomicUsize::new(0));
    let mut a = Mailbox::open(&root).unwrap();
    a.empty().unwrap();
    thread::scope(|scope| {
        scope.spawn(|| {
            let mut o = Mailbox::open(&root).unwrap();
            o.empty().unwrap();
            let mut input = Some(guard(&drops));
            observer(
                &mut o,
                &b,
                &mut input,
                || {
                    acquisitions.fetch_add(1, Ordering::SeqCst);
                    Ok(guard(&drops))
                },
                deadline,
                &mut || Ok(()),
            )
            .unwrap();
            assert!(input.is_some());
            o.current().unwrap();
        });
        scope.spawn(|| {
            let mut driver = Mailbox::open(&root).unwrap();
            driver.wait("ready", &b, deadline, &mut || Ok(())).unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 2);
            assert_eq!(acquisitions.load(Ordering::SeqCst), 0);
            driver.publish("complete", &b, deadline).unwrap();
        });
        let mut input = Some(guard(&drops));
        a.wait("observer-ended", &b, deadline, &mut || {
            input.as_mut().unwrap().quiet()
        })
        .unwrap();
        actor(
            &mut a,
            &b,
            &mut input,
            || {
                acquisitions.fetch_add(1, Ordering::SeqCst);
                Ok(guard(&drops))
            },
            deadline,
            &mut || Ok(()),
        )
        .unwrap();
        assert!(input.is_some());
        a.current().unwrap();
    });
    assert_eq!(acquisitions.load(Ordering::SeqCst), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 4);
    assert!(Mailbox::open(&root).unwrap().empty().is_err());
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn partial_stale_replaced_duplicate_and_expired_records_refuse() {
    let root = root();
    let b = binding();
    let mut mailbox = Mailbox::open(&root).unwrap();
    let leaf = Mailbox::leaf("ready").unwrap();
    write_new(&root, &leaf, &serde_json::to_vec(&b).unwrap()).unwrap();
    assert!(mailbox.receive("ready", end()).unwrap().is_none());
    write_new(&root, &format!("{leaf}.complete"), b"").unwrap();
    assert_eq!(mailbox.receive("ready", end()).unwrap(), Some(b.clone()));
    assert!(mailbox.receive("ready", end()).is_err());
    assert!(mailbox.publish("ready", &b, end()).is_err());
    fs::rename(root.join(&leaf), root.join("retained-original.json")).unwrap();
    write_new(&root, &leaf, &serde_json::to_vec(&b).unwrap()).unwrap();
    assert!(mailbox.current().is_err());
    let root = self::root();
    let mut m = Mailbox::open(&root).unwrap();
    let mut stale = b.clone();
    stale.runtime.start_ticks += 1;
    m.publish("complete", &stale, end()).unwrap();
    let mut receiver = Mailbox::open(&root).unwrap();
    assert!(receiver
        .wait("complete", &b, end(), &mut || Ok(()))
        .is_err());
    let mut receiver = Mailbox::open(&root).unwrap();
    assert!(receiver
        .wait("complete", &stale, Instant::now(), &mut || Ok(()))
        .is_err());
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn cancellation_does_not_reset_or_reacquire_guard() {
    let root = root();
    let b = binding();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut mailbox = Mailbox::open(&root).unwrap();
    let mut input = Some(guard(&drops));
    input
        .as_mut()
        .unwrap()
        .canceled
        .store(true, Ordering::SeqCst);
    assert!(observer(
        &mut mailbox,
        &b,
        &mut input,
        || -> Result<Guard> { panic!("must not acquire after cancellation") },
        end(),
        &mut || Ok(())
    )
    .is_err());
    mailbox.empty().unwrap();
    assert!(input.is_some());
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    // Completion may arrive, but contact at the single new admission still
    // refuses before observer-admitted, without a second acquisition.
    let mut peer = Mailbox::open(&root).unwrap();
    input
        .as_mut()
        .unwrap()
        .canceled
        .store(false, Ordering::SeqCst);
    let mut acquisitions = 0;
    thread::scope(|scope| {
        scope.spawn(|| {
            peer.wait("observer-ended", &b, end(), &mut || Ok(()))
                .unwrap();
            peer.publish("actor-ended", &b, end()).unwrap();
            peer.wait("ready", &b, end(), &mut || Ok(())).unwrap();
            peer.publish("complete", &b, end()).unwrap();
        });
        assert!(observer(
            &mut mailbox,
            &b,
            &mut input,
            || {
                acquisitions += 1;
                let g = guard(&drops);
                g.canceled.store(true, Ordering::SeqCst);
                Ok(g)
            },
            end(),
            &mut || Ok(())
        )
        .is_err());
    });
    assert_eq!(acquisitions, 1);
    assert!(peer.receive("observer-admitted", end()).unwrap().is_none());
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn actor_refuses_new_guard_failure_contact_and_missing_peer_without_release() {
    for fault in ["acquire", "contact", "missing-peer"] {
        let root = root();
        let b = binding();
        let drops = Arc::new(AtomicUsize::new(0));
        let mut mailbox = Mailbox::open(&root).unwrap();
        let mut peer = Mailbox::open(&root).unwrap();
        peer.publish("observer-ended", &b, end()).unwrap();
        assert_eq!(
            mailbox.receive("observer-ended", end()).unwrap(),
            Some(b.clone())
        );
        let mut input = Some(guard(&drops));
        let mut acquisitions = 0;
        let deadline = if fault == "missing-peer" {
            Instant::now() + Duration::from_secs(1)
        } else {
            end()
        };
        thread::scope(|scope| {
            scope.spawn(|| {
                peer.wait("actor-ended", &b, deadline, &mut || Ok(()))
                    .unwrap();
                peer.publish("ready", &b, deadline).unwrap();
                peer.publish("complete", &b, deadline).unwrap();
                // No observer-admitted: the actor must never release capture.
            });
            assert!(actor(
                &mut mailbox,
                &b,
                &mut input,
                || {
                    acquisitions += 1;
                    ensure!(fault != "acquire", "input acquisition failed");
                    let g = guard(&drops);
                    g.canceled.store(fault == "contact", Ordering::SeqCst);
                    Ok(g)
                },
                deadline,
                &mut || Ok(())
            )
            .is_err());
        });
        assert_eq!(acquisitions, 1, "{fault}");
        assert!(peer.receive("actor-admitted", end()).unwrap().is_none());
        if fault == "missing-peer" {
            assert!(Instant::now() >= deadline);
        }
    }
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn opening_allowance_requires_durable_matching_history_without_live_old_processes() {
    let b = binding();
    let context = |root: PathBuf| Context {
        nonce: b.nonce.clone(),
        stage: root.clone(),
        root,
        digest: b.candidate_sha256.clone(),
        pins: Vec::new(),
        own: None,
        own_snapshot: None,
        root_identity: None,
        vendor: None,
        candidate: Candidate {
            version: 2,
            nonce: b.nonce.clone(),
            kind: Kind::Native,
            fault: Fault::None,
            boot: b.boot.clone(),
            controller_sha256: "55".repeat(32),
            observer_sha256: "66".repeat(32),
            backup_sha256: "77".repeat(32),
            stock: ExpectedService {
                baseline: String::new(),
                executable_sha256: "88".repeat(32),
            },
            buddy: ExpectedService {
                baseline: String::new(),
                executable_sha256: "99".repeat(32),
            },
            source: Vec::new(),
            unit_files: Vec::new(),
            capture: None,
            companion: None,
        },
    };
    let c = context(root());
    assert!(historical_ready(&c, end()).unwrap().is_none());
    let mut peer = Mailbox::open(&c.root).unwrap();
    peer.publish("ready", &b, end()).unwrap();
    assert!(historical_ready(&c, end()).is_err());
    peer.publish("observer-ended", &b, end()).unwrap();
    assert!(historical_ready(&c, end()).is_err());
    let mut stale = b.clone();
    stale.runtime.start_ticks += 1;
    peer.publish("actor-ended", &stale, end()).unwrap();
    assert!(historical_ready(&c, end()).is_err());

    let c = context(root());
    let mut peer = Mailbox::open(&c.root).unwrap();
    for phase in ["observer-ended", "actor-ended", "ready"] {
        peer.publish(phase, &b, end()).unwrap();
    }
    let history = historical_ready(&c, end()).unwrap().unwrap();
    history.current().unwrap(); // Fake old PIDs do not need to be alive for recovery.
    let mut changed = context(c.root.clone());
    changed.digest = "aa".repeat(32);
    assert!(historical_ready(&changed, end()).is_err());
    let leaf = Mailbox::leaf("ready").unwrap();
    fs::rename(c.root.join(&leaf), c.root.join("retained-ready.json")).unwrap();
    write_new(&c.root, &leaf, &serde_json::to_vec(&b).unwrap()).unwrap();
    assert!(history.current().is_err());
}
