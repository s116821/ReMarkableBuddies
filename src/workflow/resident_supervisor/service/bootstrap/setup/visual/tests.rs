use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn end() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
fn root() -> PathBuf {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let path = PathBuf::from(format!("/root/visual-setup-test-{}", uuid::Uuid::new_v4()));
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
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
fn image() -> Image {
    let pixels = image::RgbaImage::from_pixel(4, 3, image::Rgba([11, 23, 47, 255]));
    let mut buffer = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut buffer, image::ImageFormat::Png)
        .unwrap();
    let png = buffer.into_inner();
    let hash = digest(&sha(&png)).unwrap();
    Image {
        operation: OperationId(Uuid::parse(&uuid::Uuid::new_v4().to_string()).unwrap()),
        image: HistoricalVisual::new([4, 3], 1.0, png, hash).unwrap(),
    }
}
struct Guard(Arc<AtomicUsize>);
impl InputGuard for Guard {
    fn quiet(&mut self) -> Result<()> {
        Ok(())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct OwnedTransport {
    root: PathBuf,
    deadline: Instant,
    calls: Vec<Stage>,
    fail_after: bool,
}
impl Transport for OwnedTransport {
    fn progress(&mut self, deadline: Instant) -> Result<()> {
        ensure!(deadline <= self.deadline, "deadline renewed");
        check(deadline)
    }
    fn capture(
        &mut self,
        stage: Stage,
        deadline: Instant,
        guard: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Image> {
        self.progress(deadline)?;
        guard()?;
        assert!(!self.root.join("setup.complete.json.complete").exists());
        if stage == Stage::Before {
            assert!(!self.root.join("setup.ready.json.complete").exists());
        } else {
            assert!(self
                .root
                .join("setup.visual-after-request.json.complete")
                .exists());
        }
        self.calls.push(stage);
        ensure!(
            !(stage == Stage::After && self.fail_after),
            "injected after refusal"
        );
        let image = image();
        guard()?;
        Ok(image)
    }
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn inspected_pair_precedes_fresh_input_and_never_renews_setup_clock() {
    let root = root();
    let b = binding();
    let deadline = end();
    let drops = Arc::new(AtomicUsize::new(0));
    let acquired = Arc::new(AtomicUsize::new(0));
    thread::scope(|scope| {
        scope.spawn(|| {
            let mut mailbox = Mailbox::open(&root).unwrap();
            let mut input = Some(Guard(drops.clone()));
            let mut transport = OwnedTransport {
                root: root.clone(),
                deadline,
                calls: vec![],
                fail_after: false,
            };
            observer(
                &mut mailbox,
                &b,
                &mut input,
                || {
                    acquired.fetch_add(1, Ordering::SeqCst);
                    Ok(Guard(drops.clone()))
                },
                deadline,
                &mut transport,
                &mut || Ok(()),
            )
            .unwrap();
            assert_eq!(transport.calls, [Stage::Before, Stage::After]);
            mailbox.current().unwrap();
            assert!(input.is_some());
        });
        scope.spawn(|| {
            let mut driver = Mailbox::open(&root).unwrap();
            driver.wait("ready", &b, deadline, &mut || Ok(())).unwrap();
            assert_eq!(drops.load(Ordering::SeqCst), 2);
            assert_eq!(acquired.load(Ordering::SeqCst), 0);
            driver.require_visual(Stage::Before, &b, deadline).unwrap();
            assert!(driver.require_visual_pair(&b, deadline).is_err());
            driver.validate_visual_after_request(&b, deadline).unwrap();
            driver
                .publish("visual-after-request", &b, deadline)
                .unwrap();
            assert!(driver.validate_visual_after_request(&b, deadline).is_err());
            driver
                .wait("visual-after", &b, deadline, &mut || Ok(()))
                .unwrap();
            driver.require_visual_pair(&b, deadline).unwrap();
            assert_eq!(acquired.load(Ordering::SeqCst), 0);
            driver.publish("complete", &b, deadline).unwrap();
        });
        let mut actor = Mailbox::open(&root).unwrap();
        let mut input = Some(Guard(drops.clone()));
        actor
            .wait("observer-ended", &b, deadline, &mut || Ok(()))
            .unwrap();
        super::super::actor_visual(
            &mut actor,
            &b,
            &mut input,
            || {
                acquired.fetch_add(1, Ordering::SeqCst);
                Ok(Guard(drops.clone()))
            },
            deadline,
            &mut || Ok(()),
        )
        .unwrap();
        actor.current().unwrap();
    });
    assert_eq!(acquired.load(Ordering::SeqCst), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 4);
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn images_refuse_partial_changed_wrong_scope_and_reused_operation() {
    for fault in [
        "partial",
        "stage",
        "binding",
        "operation",
        "dimensions",
        "digest",
        "png",
        "procedure",
        "extra",
        "duplicate",
    ] {
        let root = root();
        let b = binding();
        let mut sender = Mailbox::open(&root).unwrap();
        if fault == "partial" {
            write_new(&root, &Stage::Before.png(), image().image.png()).unwrap();
            assert!(sender.require_visual(Stage::Before, &b, end()).is_err());
            assert!(sender.empty().is_err());
            continue;
        }
        sender
            .publish_visual(Stage::Before, &b, image(), end(), &mut || Ok(()))
            .unwrap();
        if fault == "duplicate" {
            assert!(sender
                .publish_visual(Stage::Before, &b, image(), end(), &mut || Ok(()))
                .is_err());
            continue;
        }
        if fault == "png" {
            let path = root.join(Stage::Before.png());
            let mut bytes = fs::read(&path).unwrap();
            bytes[50] ^= 1;
            fs::write(path, bytes).unwrap();
        } else {
            let path = root.join(Stage::Before.receipt());
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match fault {
                "stage" => value["stage"] = "after".into(),
                "binding" => value["binding"]["runtime"]["start_ticks"] = 999.into(),
                "operation" => value["operation"] = "bad".into(),
                "dimensions" => value["dimensions"] = serde_json::json!([5, 3]),
                "digest" => value["sha256"] = "00".repeat(32).into(),
                "procedure" => value["procedure"] = "source".into(),
                "extra" => value["extra"] = true.into(),
                _ => unreachable!(),
            }
            fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        assert!(
            Mailbox::open(&root)
                .unwrap()
                .require_visual(Stage::Before, &b, end())
                .is_err(),
            "{fault}"
        );
        assert!(sender.current().is_err());
    }
    let root = root();
    let b = binding();
    let mut mailbox = Mailbox::open(&root).unwrap();
    let before = image();
    let operation = before.operation;
    mailbox
        .publish_visual(Stage::Before, &b, before, end(), &mut || Ok(()))
        .unwrap();
    let mut after = image();
    after.operation = operation;
    assert!(mailbox
        .publish_visual(Stage::After, &b, after, end(), &mut || Ok(()))
        .is_err());
    assert!(!root.join("setup.visual-after.json.complete").exists());
    assert!(mailbox.require_visual_pair(&b, end()).is_err());
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn failed_after_keeps_ready_history_but_never_admits_completion_or_new_input() {
    let root = root();
    let b = binding();
    let deadline = end();
    let drops = Arc::new(AtomicUsize::new(0));
    thread::scope(|scope| {
        scope.spawn(|| {
            let mut peer = Mailbox::open(&root).unwrap();
            peer.wait("observer-ended", &b, deadline, &mut || Ok(()))
                .unwrap();
            peer.publish("actor-ended", &b, deadline).unwrap();
            peer.wait("ready", &b, deadline, &mut || Ok(())).unwrap();
            peer.validate_visual_after_request(&b, deadline).unwrap();
            peer.publish("visual-after-request", &b, deadline).unwrap();
        });
        let mut mailbox = Mailbox::open(&root).unwrap();
        let mut input = Some(Guard(drops.clone()));
        let mut transport = OwnedTransport {
            root: root.clone(),
            deadline,
            calls: vec![],
            fail_after: true,
        };
        assert!(observer(
            &mut mailbox,
            &b,
            &mut input,
            || -> Result<Guard> { panic!("must not reacquire") },
            deadline,
            &mut transport,
            &mut || Ok(())
        )
        .is_err());
        assert_eq!(transport.calls, [Stage::Before, Stage::After]);
        assert!(input.is_none());
        mailbox.current().unwrap();
    });
    let mut retained = Mailbox::open(&root).unwrap();
    assert_eq!(retained.receive("ready", end()).unwrap(), Some(b.clone()));
    for phase in ["observer-ended", "actor-ended"] {
        assert_eq!(retained.receive(phase, end()).unwrap(), Some(b.clone()));
    }
    retained.require_visual(Stage::Before, &b, end()).unwrap();
    assert!(retained.require_visual_pair(&b, end()).is_err());
    for phase in [
        "complete",
        "observer-admitted",
        "actor-admitted",
        "visual-after",
    ] {
        assert!(retained.receive(phase, end()).unwrap().is_none());
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
#[ignore = "protected root filesystem; run in the owned Linux root container"]
fn admission_loss_after_image_io_and_early_completion_never_admit_input() {
    for fault in [
        "stage",
        "ready",
        "early-before",
        "early-ready",
        "early-after",
    ] {
        let root = root();
        let b = binding();
        let deadline = end();
        thread::scope(|scope| {
            scope.spawn(|| {
                let mut peer = Mailbox::open(&root).unwrap();
                peer.wait("observer-ended", &b, deadline, &mut || Ok(()))
                    .unwrap();
                peer.publish("actor-ended", &b, deadline).unwrap();
                if fault == "early-after" {
                    peer.wait("ready", &b, deadline, &mut || Ok(())).unwrap();
                    peer.publish("visual-after-request", &b, deadline).unwrap();
                }
            });
            let mut mailbox = Mailbox::open(&root).unwrap();
            let mut input = Some(Guard(Arc::new(AtomicUsize::new(0))));
            let mut transport = OwnedTransport {
                root: root.clone(),
                deadline,
                calls: vec![],
                fail_after: false,
            };
            let mut injected = false;
            let result = observer(
                &mut mailbox,
                &b,
                &mut input,
                || -> Result<Guard> { panic!("must not reacquire") },
                deadline,
                &mut transport,
                &mut || {
                    let stage_written = root.join("setup.visual-before.json").exists();
                    let ready_written = root.join("setup.ready.json").exists();
                    let inject = match fault {
                        "early-before" => stage_written,
                        "early-ready" => ready_written,
                        "early-after" => root.join("setup.visual-after.json").exists(),
                        _ => false,
                    };
                    if inject && !injected {
                        write_new(&root, "setup.complete.json", b"early")?;
                        injected = true;
                    }
                    ensure!(
                        !(fault == "stage" && stage_written || fault == "ready" && ready_written),
                        "injected closing admission loss"
                    );
                    Ok(())
                },
            );
            assert!(result.is_err(), "{fault}");
            if fault.starts_with("early-") {
                assert!(injected);
                assert!(
                    format!("{:#}", result.unwrap_err()).contains("preexisting setup record"),
                    "{fault}"
                );
            }
            assert_eq!(
                transport.calls,
                if fault == "early-after" {
                    vec![Stage::Before, Stage::After]
                } else {
                    vec![Stage::Before]
                }
            );
            assert!(input.is_none());
        });
        assert!(root.join(Stage::Before.png()).exists(), "{fault}");
        assert!(root.join(Stage::Before.receipt()).exists(), "{fault}");
        assert_eq!(
            root.join("setup.visual-before.json.complete").exists(),
            matches!(fault, "ready" | "early-ready" | "early-after")
        );
        assert_eq!(
            root.join("setup.ready.json.complete").exists(),
            fault == "early-after"
        );
        assert!(!root.join("setup.visual-after.json.complete").exists());
        for phase in ["complete", "observer-admitted", "actor-admitted"] {
            assert!(
                !root.join(format!("setup.{phase}.json.complete")).exists(),
                "{fault}"
            );
        }
    }
}
