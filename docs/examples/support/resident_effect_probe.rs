//! Development packet only. Called by the already authenticated observer parent.
use anyhow::{ensure, Context, Result};
use remarkable_open_sdk::{
    native_adapter::{InsertAfter, ReaderFacts, Reply, SelectAdjacent},
    OperationId, Uuid,
};
use remarkable_reader_buddy::{
    storage::{Store, Uuid as StoreUuid},
    workflow::{
        resident_client,
        resident_runtime::{ReadOnlyAdmission, ReadOnlyRequest, ResidentRuntime},
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const DEVELOPMENT_EFFECT_PROBE: bool = true; // Selected once for the reusable development build.

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    Insert,
    Select,
    Reuse,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePlan {
    schema: u8,
    nonce: String,
    candidate_sha256: String,
    mode: Mode,
    conversation: String,
    operation: String,
    observe_operation: String,
    navigation_operation: String,
    target_observe_operation: String,
    navigation_policy: String,
    orientation: String,
    target: String,
    document: String,
    page: String,
    order: Vec<String>,
    #[serde(default)]
    diagnostic_pen_line: Option<PenLine>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PenLine {
    operation: String,
    observe_operation: String,
    capture_operation: String,
    policy: String,
    baseline: StorageBaseline,
    close_operation: String,
    closed_observe_operation: String,
    reopen_operation: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBaseline {
    sha256: String,
    bytes: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageBaseline {
    rm: FileBaseline,
    thumbnail: FileBaseline,
}
pub struct Plan {
    wire: WirePlan,
    file: File,
    path: PathBuf,
    metadata: Metadata,
    pinned_bytes: Vec<u8>,
    sha256: String,
}
fn id(text: &str) -> Result<Uuid> {
    let value = Uuid::parse(text).context("invalid probe UUID")?;
    ensure!(value.to_string() == text, "noncanonical probe UUID");
    Ok(value)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn before(deadline: Instant) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "effect probe original deadline expired"
    );
    Ok(())
}
impl Plan {
    pub fn load(stage: &Path, nonce: &str, candidate: Option<&str>) -> Result<Option<Self>> {
        if !DEVELOPMENT_EFFECT_PROBE {
            return Ok(None);
        }
        let candidate =
            candidate.context("development effect probe requires bound capture candidate")?;
        let path = stage.join("effect-probe.json");
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(&path)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == 0
                && metadata.gid() == 0
                && metadata.mode() & 0o7777 == 0o600
                && metadata.nlink() == 1
                && metadata.len() <= 4096,
            "unsafe effect probe plan"
        );
        let mut bytes = Vec::new();
        (&mut file).take(4097).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == metadata.len(),
            "probe plan changed during load"
        );
        let sha256 = hex(&Sha256::digest(&bytes));
        let wire: WirePlan = serde_json::from_slice(&bytes)?;
        ensure!(
            wire.schema == 1 && wire.nonce == nonce && wire.candidate_sha256 == candidate,
            "foreign effect probe plan"
        );
        for value in [
            &wire.conversation,
            &wire.operation,
            &wire.observe_operation,
            &wire.navigation_operation,
            &wire.target_observe_operation,
            &wire.target,
            &wire.document,
            &wire.page,
        ] {
            id(value)?;
        }
        let operations = [
            &wire.operation,
            &wire.observe_operation,
            &wire.navigation_operation,
            &wire.target_observe_operation,
        ];
        ensure!(
            operations
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == 4,
            "probe operation IDs must differ"
        );
        ensure!(
            wire.orientation == "portrait",
            "logical gesture requires explicit portrait plan"
        );
        ensure!(
            match wire.mode {
                Mode::Insert => wire.navigation_policy == "logical-next-if-source-v1",
                Mode::Select | Mode::Reuse => wire.navigation_policy == "none",
            },
            "unsupported navigation policy"
        );
        if let Some(pen) = &wire.diagnostic_pen_line {
            ensure!(
                matches!(wire.mode, Mode::Reuse)
                    && cfg!(feature = "development-native-close")
                    && pen.policy == "rm2-known-fixed-stroke-v1",
                "pen probe requires explicit current-target reuse/close policy"
            );
            let ids = [
                &wire.operation,
                &wire.observe_operation,
                &wire.navigation_operation,
                &wire.target_observe_operation,
                &pen.operation,
                &pen.observe_operation,
                &pen.capture_operation,
                &pen.close_operation,
                &pen.closed_observe_operation,
                &pen.reopen_operation,
            ];
            ensure!(
                ids.iter().collect::<std::collections::BTreeSet<_>>().len() == 10,
                "pen/effect/close operation IDs reused"
            );
            for value in ids {
                id(value)?;
            }
            for file in [&pen.baseline.rm, &pen.baseline.thumbnail] {
                ensure!(
                    file.sha256.len() == 64
                        && file
                            .sha256
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                        && file.bytes > 0
                        && file.bytes <= 16 * 1024 * 1024,
                    "invalid pen storage baseline"
                );
            }
        }
        let plan = Self {
            wire,
            file,
            path,
            metadata,
            pinned_bytes: bytes,
            sha256,
        };
        plan.current()?;
        plan.facts()?;
        Ok(Some(plan))
    }
    pub fn navigation_binding(&self) -> Result<remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::effect::NavigationBinding>{
        remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::effect::NavigationBinding::new(
            &self.wire.nonce, &self.wire.candidate_sha256, &self.sha256, &self.wire.navigation_operation,
        )
    }
    pub fn pen_binding(&self) -> Result<remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::effect::NavigationBinding>{
        let pen = self
            .wire
            .diagnostic_pen_line
            .as_ref()
            .context("pen probe not selected")?;
        remarkable_reader_buddy::workflow::resident_supervisor::service::bootstrap::effect::NavigationBinding::pen(
            &self.wire.nonce, &self.wire.candidate_sha256, &self.sha256, &pen.operation,
        )
    }
    pub fn current(&self) -> Result<()> {
        for actual in [self.file.metadata()?, fs::symlink_metadata(&self.path)?] {
            ensure!(
                actual.dev() == self.metadata.dev()
                    && actual.ino() == self.metadata.ino()
                    && actual.mode() == self.metadata.mode()
                    && actual.uid() == 0
                    && actual.gid() == 0
                    && actual.nlink() == 1
                    && actual.len() == self.metadata.len()
                    && actual.mtime() == self.metadata.mtime()
                    && actual.mtime_nsec() == self.metadata.mtime_nsec()
                    && actual.ctime() == self.metadata.ctime()
                    && actual.ctime_nsec() == self.metadata.ctime_nsec(),
                "effect probe plan replaced or changed"
            );
        }
        let mut actual = vec![0; self.pinned_bytes.len()];
        self.file.read_exact_at(&mut actual, 0)?;
        ensure!(
            actual == self.pinned_bytes && hex(&Sha256::digest(&actual)) == self.sha256,
            "effect plan bytes changed"
        );
        Ok(())
    }
    fn facts(&self) -> Result<ReaderFacts> {
        ensure!(
            (1..=17).contains(&self.wire.order.len()),
            "probe source order bound"
        );
        ReaderFacts::new(
            remarkable_open_sdk::PageKey {
                document: id(&self.wire.document)?,
                page: id(&self.wire.page)?,
            },
            self.wire
                .order
                .iter()
                .map(|s| id(s))
                .collect::<Result<_>>()?,
        )
        .map_err(|e| anyhow::anyhow!("invalid probe source: {e:?}"))
    }
}
fn retain(root: &Path, leaf: &str, value: serde_json::Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&value)?;
    ensure!(bytes.len() <= 128 * 1024, "probe receipt bound");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(leaf))?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(())
}
static EFFECT_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn run(
    runtime: &mut ResidentRuntime,
    store: &Store,
    plan: &Plan,
    source: &ReaderFacts,
    capture_operation: OperationId,
    root: &Path,
    overall: Instant,
    admission: &mut dyn ReadOnlyAdmission,
    logical_forward: &mut dyn FnMut(&mut ResidentRuntime, &Plan, Instant) -> Result<()>,
    diagnostic_pen: &mut dyn FnMut(&mut ResidentRuntime, &Plan, Instant) -> Result<()>,
) -> Result<()> {
    ensure!(
        !EFFECT_STARTED.swap(true, std::sync::atomic::Ordering::AcqRel),
        "development effect already consumed in this process"
    );
    // One new explicitly selected request, not a renewed capture/setup clock.
    let deadline = overall.min(
        Instant::now()
            + Duration::from_secs(if matches!(plan.wire.mode, Mode::Reuse) {
                90
            } else {
                30
            }),
    );
    let operation = OperationId(id(&plan.wire.operation)?);
    let observed = OperationId(id(&plan.wire.observe_operation)?);
    ensure!(
        operation != capture_operation
            && observed != capture_operation
            && OperationId(id(&plan.wire.navigation_operation)?) != capture_operation
            && OperationId(id(&plan.wire.target_observe_operation)?) != capture_operation,
        "capture/probe operations reused"
    );
    ensure!(
        plan.facts()? == *source,
        "captured source differs from exact probe plan"
    );
    let target = id(&plan.wire.target)?;
    match plan.wire.mode {
        Mode::Insert => ensure!(
            source.order().len() <= 16 && !source.order().contains(&target),
            "insertion target present or source too large"
        ),
        Mode::Reuse => ensure!(
            target == source.page().page,
            "reuse must keep the freshly captured current target"
        ),
        Mode::Select => {
            let index = source
                .order()
                .iter()
                .position(|v| *v == source.page().page)
                .context("source absent")?;
            ensure!(
                target == source.page().page || source.order().get(index + 1) == Some(&target),
                "selection target is not current/immediate successor"
            );
        }
    }
    let conversation = StoreUuid::from_bytes(*id(&plan.wire.conversation)?.as_bytes());
    before(deadline)?;
    plan.current()?;
    retain(
        root,
        "effect-probe.intent.json",
        serde_json::json!({"schema":1,"nonce":plan.wire.nonce,"plan_sha256":plan.sha256,"operation":plan.wire.operation,"observe_operation":plan.wire.observe_operation,"source":format!("{source:?}"),"target":plan.wire.target,"runtime":format!("{:?}",runtime.identity()),"native_authority":false}),
    )?;
    let expected_order = if matches!(plan.wire.mode, Mode::Reuse) {
        ensure!(
            resident_client::attempts(store)?.is_empty(),
            "reuse process has unexpected prior mutation journal"
        );
        runtime.read_only_boundary(deadline, admission)?;
        retain(
            root,
            "effect-probe.result.json",
            serde_json::json!({"schema":1,"mode":"reuse","mutation_dispatched":false,"target":plan.wire.target,"native_authority":false}),
        )?;
        source.order().to_vec()
    } else {
        let attempted = (|| -> Result<Reply> {
            runtime.read_only_boundary(deadline, admission)?;
            before(deadline)?;
            plan.current()?;
            let reply = runtime.with_connection(
                deadline,
                &mut || {
                    before(deadline)?;
                    plan.current()?;
                    admission.pulse(deadline)
                },
                |connection, pulse| match plan.wire.mode {
                    Mode::Insert => connection
                        .insert_after_with_progress(
                            conversation,
                            operation,
                            InsertAfter {
                                source: source.clone(),
                                target,
                            },
                            deadline,
                            pulse,
                        )
                        .map_err(anyhow::Error::new),
                    Mode::Reuse => {
                        anyhow::bail!("reuse never dispatches an insertion or selection")
                    }
                    Mode::Select => connection
                        .select_adjacent_with_progress(
                            conversation,
                            operation,
                            SelectAdjacent {
                                expected: source.clone(),
                                target,
                            },
                            deadline,
                            pulse,
                        )
                        .map_err(anyhow::Error::new),
                },
            )?;
            runtime.read_only_boundary(deadline, admission)?;
            Ok(reply)
        })();
        // Persist all outcomes before Drop/shutdown; Store owns original exact wire journal.
        let journal=resident_client::attempts(store).map(|attempts|attempts.into_iter().map(|a|serde_json::json!({"conversation":a.conversation.to_string(),"request_sha256":hex(&a.request.fingerprint()),"request_bytes":hex(a.request.encoded()),"known_not_dispatched":a.known_not_dispatched,"reply":format!("{:?}",a.reply)})).collect::<Vec<_>>());
        let journal_inspected = journal.is_ok();
        retain(
            root,
            "effect-probe.result.json",
            serde_json::json!({"schema":1,"operation":plan.wire.operation,"result":format!("{attempted:?}"),"journal":format!("{journal:?}"),"native_authority":false}),
        )?;
        ensure!(
            journal_inspected,
            "journal inspection failed; no further action"
        );
        let reply = attempted?;
        let expected_order = match (&plan.wire.mode, &reply) {
            (
                Mode::Insert,
                Reply::InsertionObserved {
                    target: actual,
                    after_order,
                },
            ) => {
                ensure!(
                    actual.document == source.page().document && actual.page == target,
                    "foreign inserted target"
                );
                let expected = InsertAfter {
                    source: source.clone(),
                    target,
                }
                .expected_after_order();
                ensure!(*after_order == expected, "wrong insertion order");
                expected
            }
            (Mode::Select, Reply::TargetObserved(facts)) => {
                ensure!(
                    facts.page().document == source.page().document
                        && facts.page().page == target
                        && facts.order() == source.order(),
                    "wrong selected target/order"
                );
                source.order().to_vec()
            }
            _ => anyhow::bail!("probe refused/busy/uncertain; no further action: {reply:?}"),
        };
        expected_order
    };
    let fresh = runtime.request_read_only(observed, ReadOnlyRequest::Observe, deadline, admission);
    retain(
        root,
        "effect-probe.observe.json",
        serde_json::json!({"schema":1,"operation":plan.wire.observe_operation,"result":format!("{fresh:?}"),"native_authority":false}),
    )?;
    match fresh? {
        Reply::Observed(facts) => {
            ensure!(
                facts.page().document == source.page().document
                    && facts.order() == expected_order.as_slice(),
                "fresh post-effect order/document mismatch"
            );
            match plan.wire.mode {
                Mode::Select | Mode::Reuse => {
                    ensure!(facts.page().page == target, "target not freshly observed")
                }
                Mode::Insert => {
                    if facts.page().page == target {
                        retain(
                            root,
                            "effect-probe.navigation.result.json",
                            serde_json::json!({"schema":1,"operation":plan.wire.navigation_operation,"outcome":"skipped-target-already-active","gesture_attempted":false,"target":plan.wire.target,"native_authority":false}),
                        )?;
                        navigation_verified(root, &plan.wire.navigation_operation, &facts)?;
                    } else {
                        ensure!(
                            facts.page().page == source.page().page,
                            "unexpected active page after insertion"
                        );
                        let position = facts
                            .order()
                            .iter()
                            .position(|id| *id == facts.page().page)
                            .context("active source absent")?;
                        ensure!(
                            facts.order().get(position + 1) == Some(&target),
                            "inserted target is not immediate logical successor"
                        );
                        retain(
                            root,
                            "effect-probe.navigation.intent.json",
                            serde_json::json!({"schema":1,"operation":plan.wire.navigation_operation,"source":format!("{facts:?}"),"target":plan.wire.target,"policy":"one-supported-logical-next-v1","status":"consumed-before-contact","physical_input_window":"explicitly unobserved own gesture; no continuous-input/write authority inferred","native_authority":false}),
                        )?;
                        let dispatched = logical_forward(runtime, plan, deadline);
                        retain(
                            root,
                            "effect-probe.navigation.result.json",
                            serde_json::json!({"schema":1,"operation":plan.wire.navigation_operation,"outcome":format!("{dispatched:?}"),"gesture_attempted":true,"selection_verified":false,"native_authority":false}),
                        )?;
                        dispatched?;
                        let selected = runtime.request_read_only(
                            OperationId(id(&plan.wire.target_observe_operation)?),
                            ReadOnlyRequest::Observe,
                            deadline,
                            admission,
                        );
                        retain(
                            root,
                            "effect-probe.target-observe.json",
                            serde_json::json!({"schema":1,"operation":plan.wire.target_observe_operation,"result":format!("{selected:?}"),"native_authority":false}),
                        )?;
                        let Reply::Observed(selected) = selected? else {
                            anyhow::bail!("fresh target observation refused; no retry/write")
                        };
                        ensure!(
                            selected.page().document == source.page().document
                                && selected.page().page == target
                                && selected.order() == expected_order.as_slice(),
                            "logical gesture did not freshly confirm exact target/order; no retry/write"
                        );
                        navigation_verified(root, &plan.wire.navigation_operation, &selected)?;
                    }
                }
            }
        }
        other => anyhow::bail!("fresh post-effect observation refused: {other:?}"),
    }
    if let Some(pen) = &plan.wire.diagnostic_pen_line {
        ensure!(
            OperationId(id(&pen.operation)?) != capture_operation
                && OperationId(id(&pen.observe_operation)?) != capture_operation
                && OperationId(id(&pen.capture_operation)?) != capture_operation,
            "pen/capture operation reused"
        );
        let fresh = runtime.request_read_only(
            OperationId(id(&pen.observe_operation)?),
            ReadOnlyRequest::Observe,
            deadline,
            admission,
        );
        retain(
            root,
            "effect-probe.pen.observe.json",
            serde_json::json!({"schema":1,"operation":pen.observe_operation,"result":format!("{fresh:?}"),"native_authority":false}),
        )?;
        let Reply::Observed(target_facts) = fresh? else {
            anyhow::bail!("pen target observation refused; no contact")
        };
        ensure!(
            target_facts.page().document == source.page().document
                && target_facts.page().page == target
                && target_facts.order() == expected_order.as_slice(),
            "pen requires exact fresh target/order"
        );
        let storage =
            super::storage_probe::StorageWatch::open(&plan.wire.document, &plan.wire.target)?;
        let before_storage = storage
            .sample(deadline)?
            .context("target storage changed before pen contact")?;
        ensure!(
            before_storage.rm.sha256 == pen.baseline.rm.sha256
                && before_storage.rm.bytes == pen.baseline.rm.bytes
                && before_storage.thumbnail.sha256 == pen.baseline.thumbnail.sha256
                && before_storage.thumbnail.bytes == pen.baseline.thumbnail.bytes,
            "target storage does not match fresh backup before contact"
        );
        retain(
            root,
            "effect-probe.pen.before-storage.json",
            serde_json::json!({"schema":1,"nonce":plan.wire.nonce,"candidate_sha256":plan.wire.candidate_sha256,"plan_sha256":plan.sha256,"operation":pen.operation,"document":plan.wire.document,"target":plan.wire.target,"state":before_storage,"phase":"before-contact","native_authority":false}),
        )?;
        retain(
            root,
            "effect-probe.pen.intent.json",
            serde_json::json!({"schema":1,"operation":pen.operation,"policy":pen.policy,"target":plan.wire.target,"status":"consumed-before-contact","raw_stroke":[[11138,7371],[11056,7616]],"geometry_basis":"exact REM57 reader_fixed_stroke_policy, 56 frames/178 records","physical_input_window":"explicitly unobserved own pen contact; no Writer lease or continuous input authority","native_authority":false}),
        )?;
        let contacted = diagnostic_pen(runtime, plan, deadline);
        retain(
            root,
            "effect-probe.pen.result.json",
            serde_json::json!({"schema":1,"operation":pen.operation,"result":format!("{contacted:?}"),"ink_or_save_verified":false,"native_authority":false}),
        )?;
        contacted?;
        let captured = runtime.request_read_only_capture(
            OperationId(id(&pen.capture_operation)?),
            ReadOnlyRequest::CaptureSource(target_facts.clone()),
            deadline,
            admission,
        );
        let result = match &captured {
            Ok(Reply::Captured(image)) => {
                serde_json::json!({"captured":true,"dimensions":image.dimensions(),"png_bytes":image.png().len(),"sha256":hex(&image.sha256()),"before":format!("{:?}",image.before()),"after":format!("{:?}",image.after())})
            }
            other => serde_json::json!({"captured":false,"result":format!("{other:?}")}),
        };
        retain(
            root,
            "effect-probe.pen.capture.json",
            serde_json::json!({"schema":1,"operation":pen.capture_operation,"result":result,"native_authority":false}),
        )?;
        let Reply::Captured(image) = captured? else {
            anyhow::bail!("post-pen capture refused; no retry")
        };
        ensure!(
            image.before().page().page == target
                && image.after().page().page == target
                && image.before().order() == expected_order.as_slice()
                && image.after().order() == expected_order.as_slice(),
            "post-pen capture target/order changed"
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("effect-probe.pen.capture.png"))?;
        file.write_all(image.png())?;
        file.sync_all()?;
        File::open(root)?.sync_all()?;
        close_and_sample(
            runtime,
            plan,
            pen,
            image.after().clone(),
            conversation,
            root,
            deadline,
            admission,
            &storage,
            &before_storage,
        )?;
    }
    before(deadline)?;
    plan.current()
}

#[cfg(feature = "development-native-close")]
#[allow(clippy::too_many_arguments)]
fn close_and_sample(
    runtime: &mut ResidentRuntime,
    plan: &Plan,
    pen: &PenLine,
    expected: ReaderFacts,
    conversation: StoreUuid,
    root: &Path,
    deadline: Instant,
    admission: &mut dyn ReadOnlyAdmission,
    storage: &super::storage_probe::StorageWatch,
    before_storage: &super::storage_probe::Samples,
) -> Result<()> {
    plan.current()?;
    before(deadline)?;
    retain(
        root,
        "effect-probe.pen.close.intent.json",
        serde_json::json!({"schema":1,"operation":pen.close_operation,"expected":format!("{expected:?}"),"status":"one close selected; durable reserved/unknown wire journal precedes dispatch","save_verified":false,"native_authority":false}),
    )?;
    let close = (|| -> Result<Reply> {
        runtime.read_only_boundary(deadline, admission)?;
        runtime.with_connection(
            deadline,
            &mut || {
                plan.current()?;
                admission.pulse(deadline)
            },
            |connection, pulse| {
                connection.close_ordinary_with_progress(
                    conversation,
                    OperationId(id(&pen.close_operation)?),
                    expected.clone(),
                    deadline,
                    pulse,
                )
            },
        )
    })();
    retain(
        root,
        "effect-probe.pen.close.result.json",
        serde_json::json!({"schema":1,"operation":pen.close_operation,"result":format!("{close:?}"),"save_verified":false,"native_authority":false}),
    )?;
    ensure!(
        matches!(close?, Reply::OrdinaryCloseDispatched),
        "ordinary close not positively dispatched; no retry"
    );
    let closed = runtime.request_read_only(
        OperationId(id(&pen.closed_observe_operation)?),
        ReadOnlyRequest::ObserveClosed,
        deadline,
        admission,
    );
    retain(
        root,
        "effect-probe.pen.closed-observe.json",
        serde_json::json!({"schema":1,"operation":pen.closed_observe_operation,"result":format!("{closed:?}"),"save_verified":false,"native_authority":false}),
    )?;
    ensure!(
        matches!(closed?, Reply::ClosedObserved),
        "closed view was not separately observed; no retry"
    );
    // Reproduce the known native sequence: reopen in the retained Reader before
    // sampling storage. Reopening is a fixture action, never product navigation.
    plan.current()?;
    retain(
        root,
        "effect-probe.pen.reopen.intent.json",
        serde_json::json!({"schema":1,"operation":pen.reopen_operation,"expected":format!("{expected:?}"),"save_verified":false,"fixture_only":true}),
    )?;
    let reopened = (|| -> Result<Reply> {
        runtime.read_only_boundary(deadline, admission)?;
        runtime.with_connection(
            deadline,
            &mut || {
                plan.current()?;
                admission.pulse(deadline)
            },
            |connection, pulse| {
                connection.reopen_fixture_with_progress(
                    conversation,
                    OperationId(id(&pen.reopen_operation)?),
                    expected.clone(),
                    deadline,
                    pulse,
                )
            },
        )
    })();
    retain(
        root,
        "effect-probe.pen.reopen.result.json",
        serde_json::json!({"schema":1,"operation":pen.reopen_operation,"result":format!("{reopened:?}"),"save_verified":false,"fixture_only":true}),
    )?;
    ensure!(
        matches!(reopened?, Reply::FixtureReopenDispatched),
        "fixture reopen not positively dispatched; no retry"
    );
    let observe_operation = OperationId(
        Uuid::parse(&uuid::Uuid::new_v4().to_string()).context("fresh reopen observation UUID")?,
    );
    let observed = runtime.request_read_only(
        observe_operation,
        ReadOnlyRequest::Observe,
        deadline,
        admission,
    );
    retain(
        root,
        "effect-probe.pen.reopen.observe.json",
        serde_json::json!({"schema":1,"operation":format!("{observe_operation:?}"),"result":format!("{observed:?}"),"save_verified":false}),
    )?;
    let Reply::Observed(facts) = observed? else {
        anyhow::bail!("reopened fixture observation refused")
    };
    ensure!(
        facts.page() == expected.page() && facts.order() == expected.order(),
        "fixture reopen target/order changed"
    );
    let capture_operation = OperationId(
        Uuid::parse(&uuid::Uuid::new_v4().to_string()).context("fresh reopen capture UUID")?,
    );
    let captured = runtime.request_read_only_capture(
        capture_operation,
        ReadOnlyRequest::CaptureSource(facts),
        deadline,
        admission,
    );
    let result = match &captured {
        Ok(Reply::Captured(image)) => {
            serde_json::json!({"captured":true,"dimensions":image.dimensions(),"png_bytes":image.png().len(),"sha256":hex(&image.sha256()),"before":format!("{:?}",image.before()),"after":format!("{:?}",image.after())})
        }
        other => serde_json::json!({"captured":false,"result":format!("{other:?}")}),
    };
    retain(
        root,
        "effect-probe.pen.reopen.capture.json",
        serde_json::json!({"schema":1,"operation":format!("{capture_operation:?}"),"result":result,"save_verified":false}),
    )?;
    let Reply::Captured(image) = captured? else {
        anyhow::bail!("reopened fixture capture refused")
    };
    ensure!(
        image.before().page() == expected.page()
            && image.after().page() == expected.page()
            && image.before().order() == expected.order()
            && image.after().order() == expected.order(),
        "reopened capture target/order changed"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("effect-probe.pen.reopen.capture.png"))?;
    file.write_all(image.png())?;
    file.sync_all()?;
    File::open(root)?.sync_all()?;
    let changed = storage.changed_quiescent(before_storage, deadline, &mut || {
        plan.current()?;
        admission.pulse(deadline)?;
        runtime.pulse()
    });
    let result = match &changed {
        Ok(state) => {
            serde_json::json!({"serialized_file_changed":true,"state":state,"quiescent_ms":250})
        }
        Err(error) => {
            serde_json::json!({"serialized_file_changed":false,"error":format!("{error:#}")})
        }
    };
    retain(
        root,
        "effect-probe.pen.save.json",
        serde_json::json!({"schema":1,"operation":pen.close_operation,"result":result,"basis":"sampled native file change/quiescence after closed view; not a save-completion ack or reopen proof","native_authority":false}),
    )?;
    // A failed sampled-file wait remains evidence. The cold reopen and archive
    // comparison independently decide whether the stroke persisted.
    Ok(())
}
#[cfg(not(feature = "development-native-close"))]
#[allow(clippy::too_many_arguments)]
fn close_and_sample(
    _: &mut ResidentRuntime,
    _: &Plan,
    _: &PenLine,
    _: ReaderFacts,
    _: StoreUuid,
    _: &Path,
    _: Instant,
    _: &mut dyn ReadOnlyAdmission,
    _: &super::storage_probe::StorageWatch,
    _: &super::storage_probe::Samples,
) -> Result<()> {
    anyhow::bail!("development-native-close feature required")
}

pub fn retain_pen_events(
    root: &Path,
    plan: &Plan,
    phase: &str,
    mut value: serde_json::Value,
) -> Result<()> {
    ensure!(
        ["before", "after"].contains(&phase),
        "invalid pen trace phase"
    );
    let fields = value.as_object_mut().context("invalid pen trace receipt")?;
    fields.insert("schema".into(), serde_json::json!(1));
    fields.insert("nonce".into(), serde_json::json!(plan.wire.nonce));
    fields.insert(
        "candidate_sha256".into(),
        serde_json::json!(plan.wire.candidate_sha256),
    );
    fields.insert("plan_sha256".into(), serde_json::json!(plan.sha256));
    fields.insert(
        "operation".into(),
        serde_json::json!(
            plan.wire
                .diagnostic_pen_line
                .as_ref()
                .context("pen not selected")?
                .operation
        ),
    );
    retain(
        root,
        &format!("effect-probe.pen.events.{phase}.json"),
        value,
    )
}

fn navigation_verified(root: &Path, operation: &str, facts: &ReaderFacts) -> Result<()> {
    retain(
        root,
        "effect-probe.navigation.verified.json",
        serde_json::json!({
            "schema":1,"operation":operation,"selection_verified":true,
            "document":facts.page().document.to_string(),"target":facts.page().page.to_string(),
            "order":facts.order().iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "basis":"fresh exact target/order Observe; no Writer lease or output authority",
            "native_authority":false
        }),
    )
}

pub fn capture_ready(
    root: &Path,
    image: &remarkable_open_sdk::native_adapter::HistoricalCapture,
) -> Result<()> {
    retain(
        root,
        "effect-probe.capture.json",
        serde_json::json!({"schema":1,"before":format!("{:?}",image.before()),"after":format!("{:?}",image.after()),"sha256":hex(&image.sha256()),"png_bytes":image.png().len(),"dimensions":image.dimensions(),"native_authority":false}),
    )
}

/// Exact SDK geometry and existing strict touch emitter; no UI-selection helper.
#[cfg(feature = "development-input-diagnostics")]
pub fn forward_once(deadline: Instant, check: &mut dyn FnMut() -> Result<()>) -> Result<()> {
    use remarkable_open_sdk::navigation::{
        gesture::{plan_gesture, Orientation, TabletModel},
        LogicalDirection,
    };
    use remarkable_reader_buddy::device::touch::{DiagnosticGestureEmitter, Touch};
    struct Emitter {
        deadline: Instant,
    }
    impl DiagnosticGestureEmitter for Emitter {
        fn emit(&mut self, device: &mut evdev::Device, events: &[evdev::InputEvent]) -> Result<()> {
            let release = events.len() == 3
                && events[1].event_type().0 == 3
                && events[1].code() == 57
                && events[1].value() == -1;
            if !release {
                before(self.deadline)?;
            }
            device.send_events(events)?;
            Ok(())
        }
    }
    let geometry = plan_gesture(
        TabletModel::Remarkable2,
        Orientation::Portrait,
        LogicalDirection::Next,
    )
    .map_err(|e| anyhow::anyhow!("unsupported logical gesture: {e:?}"))?;
    let mut touch = Touch::diagnostic_scoped_gesture(32761, Box::new(Emitter { deadline }))?;
    let result = (|| -> Result<()> {
        check()?;
        before(deadline)?;
        touch.touch_start(geometry.start())?;
        std::thread::sleep(geometry.hold_duration());
        for &point in geometry.movements() {
            check()?;
            before(deadline)?;
            touch.goto_xy(point)?;
            std::thread::sleep(geometry.movement_interval());
        }
        Ok(())
    })();
    // Release is cleanup and is always attempted, even after original expiry.
    let released = touch.touch_stop();
    match (result, released) {
        (Err(error), Err(release)) => {
            Err(error.context(format!("logical gesture release also failed: {release:#}")))
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(()), Ok(())) => {
            before(deadline)?;
            check()
        }
    }
}
#[cfg(not(feature = "development-input-diagnostics"))]
pub fn forward_once(_: Instant, _: &mut dyn FnMut() -> Result<()>) -> Result<()> {
    anyhow::bail!("explicit development-input-diagnostics build is required for logical Forward")
}
