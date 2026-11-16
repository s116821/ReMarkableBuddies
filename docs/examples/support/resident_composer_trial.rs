//! Finite, explicitly compiled tablet mechanics trial through the admitted Buddy
//! runtime. Return is a diagnostic event, not a product Submit mapping. The
//! independent bootstrap controller still owns recovery. No effect is retried.
#[path = "resident_composer_trial/armed.rs"]
mod armed;
use anyhow::{bail, ensure, Context, Result};
use remarkable_open_sdk::{
    native_adapter::{composer as wire, HistoricalCapture, LostRequest, ReaderFacts, Reply},
    OperationId,
};
use remarkable_reader_buddy::{
    conversation::{
        BindingAcquisition, BindingReceipt, Ledger, Mode, Outcome, Role, SourceObservation, Turn,
    },
    device::{
        input_observer::InputObserver,
        interaction::Interaction,
        request_input::{InputProgress, RequestInput},
        touch::TriggerCorner,
    },
    storage::{Namespace, Store, Uuid},
    workflow::{
        resident_client::{self, composer::RetainedUser},
        resident_runtime::ResidentRuntime,
        resident_supervisor::service::bootstrap::{composer::ObserverPermit, setup::ObserverSetup},
    },
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TEXT: &str = "composer trial";
const RETURN: i32 = 0x0100_0004;
const ENTER: i32 = 0x0100_0005;

pub fn validate_pins(profile: &str, adapter: &str) -> Result<()> {
    for pin in [profile, adapter] {
        ensure!(
            pin.len() == 64
                && pin
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "composer trial requires reviewed compile-time native artifact pins"
        );
        ensure!(
            pin.bytes().any(|b| b != b'0'),
            "empty composer artifact pin"
        );
    }
    Ok(())
}

/// This nonce-owned persistent Store is part of the trial's retained evidence,
/// not the product Store. No binaries or credentials are copied into it.
pub fn prepare_store(stage: &Path) -> Result<PathBuf> {
    let parent = fs::symlink_metadata(stage)?;
    ensure!(
        parent.is_dir()
            && parent.uid() == 0
            && parent.mode() & 0o22 == 0
            && fs::canonicalize(stage)? == stage,
        "unsafe composer trial parent"
    );
    let directory = File::open(stage)?;
    let actual = directory.metadata()?;
    ensure!(
        (actual.dev(), actual.ino()) == (parent.dev(), parent.ino()),
        "composer parent changed"
    );
    let mut space = std::mem::MaybeUninit::<nix::libc::statvfs>::uninit();
    // SAFETY: live fd and correctly sized writable statvfs storage.
    ensure!(
        unsafe { nix::libc::fstatvfs(directory.as_raw_fd(), space.as_mut_ptr()) } == 0,
        "composer space admission failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: successful fstatvfs initialized the result.
    let space = unsafe { space.assume_init() };
    ensure!(
        u128::from(space.f_bavail) * u128::from(space.f_frsize) >= 32 * 1024 * 1024,
        "composer trial requires 32 MiB free on its persistent filesystem"
    );
    let root = stage.join("composer-store");
    DirBuilder::new().mode(0o700).create(&root)?; // Refuse every resumed/spent directory.
    File::open(stage)?.sync_all()?;
    Ok(root)
}

fn retain(root: &Path, name: &str, value: serde_json::Value) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join(name))?;
    file.write_all(&serde_json::to_vec_pretty(&value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(root)?.sync_all()?;
    println!("composer_trial_evidence={}", root.join(name).display());
    Ok(())
}
fn fresh() -> OperationId {
    OperationId(
        remarkable_open_sdk::Uuid::parse(&Uuid::new_v4().to_string())
            .expect("fresh canonical UUID"),
    )
}
fn millis() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

enum Call<'a> {
    Capture(ReaderFacts),
    Provision(ReaderFacts),
    Read(wire::RangeIdentity, Option<Vec<i32>>),
    AwaitDiagnosed(wire::RangeIdentity, Vec<i32>),
    Clear(&'a RetainedUser),
}
struct Trial<'a> {
    setup: &'a ObserverSetup,
    root: &'a Path,
    store: &'a Arc<Store>,
    conversation: Uuid,
    overall: Instant,
    input: &'a mut TrialInput,
}
struct TrialInput {
    quiet: Option<RequestInput>,
    typing: Option<InputObserver>,
    canceled: bool,
}
impl TrialInput {
    fn finish_typing(&mut self) -> Result<()> {
        ensure!(!self.canceled, "composer input ownership was canceled");
        let result = self.finish_typing_inner();
        self.canceled |= result.is_err();
        result
    }
    fn finish_typing_inner(&mut self) -> Result<()> {
        if let Some(quiet) = &mut self.quiet {
            ensure!(
                quiet.poll()? == InputProgress::Quiet,
                "composer quiet ownership was canceled"
            );
            return Ok(());
        }
        // The old observer stays alive through creation and both polls of its
        // replacement. Reopening cannot erase a queued gesture or input loss.
        let mut fresh = RequestInput::new(TriggerCorner::LowerLeft, None)?;
        let typing = self
            .typing
            .as_mut()
            .context("missing composer typing ownership")?;
        let drained = typing.poll()?;
        ensure!(
            drained
                .iter()
                .all(|event| *event == Interaction::Invalidated)
                && typing.quiescent()
                && fresh.poll()? == InputProgress::Quiet
                && typing.poll()?.is_empty()
                && typing.quiescent()
                && fresh.poll()? == InputProgress::Quiet,
            "composer observer readmission lost input or queued cancellation"
        );
        self.quiet = Some(fresh);
        drop(self.typing.take());
        Ok(())
    }
}
impl Trial<'_> {
    fn check(&mut self, deadline: Instant) -> Result<()> {
        ensure!(
            !self.input.canceled,
            "composer input ownership was canceled"
        );
        ensure!(
            Instant::now() < deadline && Instant::now() < self.overall,
            "composer trial deadline expired"
        );
        self.setup.current()?;
        self.setup.services().pulse()?;
        ensure!(
            !self.root.join("cancel").try_exists()?,
            "composer trial canceled"
        );
        if let Some(input) = &mut self.input.quiet {
            ensure!(
                input.poll()? == InputProgress::Quiet,
                "composer quiet phase canceled by input"
            );
        } else {
            let events = self
                .input
                .typing
                .as_mut()
                .context("composer input ownership absent")?
                .poll()?;
            // Invalidated includes ordinary folio/pen activity. This one finite
            // phase is explicitly not quiet. Observer errors remain fatal; the
            // corner Reader/Undo/Redo gestures and explicit cancel file abort.
            self.input.canceled |= events
                .iter()
                .any(|event| *event != Interaction::Invalidated);
            ensure!(!self.input.canceled, "composer trial canceled by gesture");
        }
        Ok(())
    }
    fn call(
        &mut self,
        runtime: &mut ResidentRuntime,
        phase: &str,
        operation: OperationId,
        call: Call<'_>,
    ) -> Result<Reply> {
        self.call_until(runtime, phase, operation, call, self.overall)
    }
    fn call_until(
        &mut self,
        runtime: &mut ResidentRuntime,
        phase: &str,
        operation: OperationId,
        call: Call<'_>,
        limit: Instant,
    ) -> Result<Reply> {
        let deadline = self
            .overall
            .min(limit)
            .min(Instant::now() + Duration::from_secs(25));
        let witness = match &call {
            Call::AwaitDiagnosed(identity, keys) => {
                Some(std::cell::RefCell::new(armed::Witness::open(
                    runtime.evidence_directory(),
                    runtime.identity().clone(),
                    operation,
                    identity.clone(),
                    keys.clone(),
                )?))
            }
            _ => None,
        };
        let evidence = self.root.to_owned();
        self.check(deadline)?;
        self.setup
            .services()
            .boundary(deadline, &mut || self.check(deadline))?;
        retain(
            self.root,
            &format!("{phase}.intent.json"),
            serde_json::json!({
            "operation":operation.0.to_string(), "conversation":self.conversation.to_string(),
            "runtime":format!("{:?}",runtime.identity()), "product_submit_selected":false}),
        )?;
        let conversation = self.conversation;
        let result = runtime.with_connection(
            deadline,
            &mut || {
                self.check(deadline)?;
                if let Some(witness) = &witness {
                    if let Some(receipt) = witness.borrow_mut().poll()? {
                        // The observer stays quiet until the exact attempt
                        // armed. The actor permits the finite seed/input phase.
                        self.invite_typing(operation, receipt, deadline)?;
                    }
                }
                Ok(())
            },
            |connection, pulse| match call {
                Call::Capture(facts) => {
                    connection.capture_source_with_progress(operation, facts, deadline, pulse)
                }
                Call::Provision(facts) => connection.composer_provision_with_progress(
                    conversation,
                    operation,
                    facts,
                    deadline,
                    pulse,
                ),
                Call::Read(identity, keys) => connection.composer_read_with_progress(
                    conversation,
                    operation,
                    identity,
                    keys,
                    deadline,
                    pulse,
                ),
                Call::AwaitDiagnosed(identity, keys) => connection
                    .composer_await_key_diagnosed_with_progress(
                        conversation,
                        operation,
                        identity,
                        keys,
                        deadline,
                        &mut |frame, digest| {
                            witness
                                .as_ref()
                                .context("diagnostic witness absent")?
                                .borrow_mut()
                                .prepare(frame, digest)?;
                            retain(
                                &evidence,
                                &format!("{phase}.prepared.json"),
                                serde_json::json!({
                            "operation":frame.operation.0.to_string(),"request_sha256":digest,
                            "frame":format!("{frame:?}"),"sent_or_armed":false}),
                            )
                        },
                        pulse,
                    ),
                Call::Clear(retained) => {
                    connection.composer_clear_with_progress(operation, retained, deadline, pulse)
                }
            },
        );
        let journal = resident_client::attempts(self.store).map(|rows| {
            rows.into_iter()
                .map(|row| {
                    serde_json::json!({"operation":row.request.frame().operation.0.to_string(),
                "request_sha256":format!("{:x?}",row.request.fingerprint()),
                "known_not_dispatched":row.known_not_dispatched,"reply":format!("{:?}",row.reply)})
                })
                .collect::<Vec<_>>()
        });
        let result_summary = match &result {
            Ok(Reply::Captured(image)) => format!(
                "Captured {{ before: {:?}, after: {:?}, sha256: {:?}, bytes: {} }}",
                image.before(),
                image.after(),
                image.sha256(),
                image.png().len()
            ),
            _ => format!("{result:?}"),
        };
        retain(
            self.root,
            &format!("{phase}.result.json"),
            serde_json::json!({
            "operation":operation.0.to_string(),"result":result_summary,
            "journal":format!("{journal:?}"),"render_or_save_ack":false}),
        )?;
        journal?;
        if result.is_ok() {
            if let Some(witness) = &witness {
                ensure!(
                    witness.borrow().seen(),
                    "native AwaitKey never supplied its matching armed witness"
                );
            }
        }
        result
    }
    fn read(
        &mut self,
        runtime: &mut ResidentRuntime,
        phase: &str,
        identity: &wire::RangeIdentity,
    ) -> Result<wire::Draft> {
        self.read_until(runtime, phase, identity, self.overall)
    }
    fn read_until(
        &mut self,
        runtime: &mut ResidentRuntime,
        phase: &str,
        identity: &wire::RangeIdentity,
        limit: Instant,
    ) -> Result<wire::Draft> {
        match self.call_until(
            runtime,
            phase,
            fresh(),
            Call::Read(identity.clone(), None),
            limit,
        )? {
            Reply::Composer(wire::Reply::Observed(wire::Outcome::Verified { value, .. })) => {
                Ok(value)
            }
            other => bail!("{phase} did not verify: {other:?}"),
        }
    }
    fn capture(
        &mut self,
        runtime: &mut ResidentRuntime,
        phase: &str,
        facts: &ReaderFacts,
    ) -> Result<()> {
        let image = match self.call(runtime, phase, fresh(), Call::Capture(facts.clone()))? {
            Reply::Captured(image) => image,
            other => bail!("{phase} did not capture selected page: {other:?}"),
        };
        ensure!(
            image.before() == facts && image.after() == facts,
            "capture selected page changed"
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.root.join(format!("{phase}.png")))?;
        file.write_all(image.png())?;
        file.sync_all()?;
        File::open(self.root)?.sync_all()?;
        Ok(())
    }
    fn allow_typing(&mut self) -> Result<()> {
        let mut typing = InputObserver::new(TriggerCorner::LowerLeft, None)?;
        ensure!(
            typing.poll()?.is_empty() && typing.quiescent(),
            "input before composer typing handoff"
        );
        self.check(self.overall)?;
        self.input.typing = Some(typing);
        drop(self.input.quiet.take());
        Ok(())
    }
    fn invite_typing(
        &mut self,
        operation: OperationId,
        receipt: armed::Receipt,
        deadline: Instant,
    ) -> Result<()> {
        receipt.current()?;
        self.allow_typing()?;
        let staged = self.root.join("typing-cue.staged.json");
        let ready = self.root.join("typing-ready.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged)?;
        let value = serde_json::json!({
            "operation":operation.0.to_string(),"native_armed_at_sample":receipt.line,
            "native_expiry_ns":receipt.expiry_ns,"text":TEXT,
            "diagnostic_key":"Return once; ordinary newline is preserved",
            "live_lease_or_submit_claim":false});
        file.write_all(&serde_json::to_vec_pretty(&value)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        File::open(self.root)?.sync_all()?;
        // Slow rebinding, service checks or staging fsync must not turn a valid
        // sampled receipt into an expired operator cue. Publish one complete
        // already-durable inode atomically without overwriting an earlier cue.
        self.check(deadline)?;
        receipt.current()?;
        fs::hard_link(&staged, &ready)?;
        File::open(self.root)?.sync_all()?;
        println!("composer_trial_evidence={}", ready.display());
        Ok(())
    }
    fn require_quiet_again(&mut self) -> Result<()> {
        self.check(self.overall)?;
        self.input.finish_typing()
    }
}

/// Source/page association is historical. It uses the existing Ledger schema;
/// neither the capture nor its stored metadata supplies a native revision lease.
fn bind(
    ledger: &Ledger,
    conversation: Uuid,
    setup: &ObserverSetup,
    image: &HistoricalCapture,
    runtime: &ResidentRuntime,
) -> Result<()> {
    let facts = image.after();
    ensure!(
        *facts == setup.plan.facts()?,
        "composer capture differs from selected trial page"
    );
    let opening = setup
        .plan
        .opening_metadata
        .as_ref()
        .context("composer trial requires retained opening metadata")?;
    let revision = format!(
        "opening-content-sha256:{:x}",
        Sha256::digest(opening.content.as_bytes())
    );
    let document = Uuid::from_bytes(*facts.page().document.as_bytes());
    let page = Uuid::from_bytes(*facts.page().page.as_bytes());
    let order: Vec<_> = facts
        .order()
        .iter()
        .map(|id| Uuid::from_bytes(*id.as_bytes()))
        .collect();
    let operation = Uuid::new_v4();
    let now = millis()?;
    ledger.create(conversation, Uuid::new_v4(), now)?;
    ledger.bind(operation, ledger.expected(Namespace::Conversation, conversation)?,
        ledger.expected(Namespace::Conversation, Ledger::binding_id(document,page))?, BindingReceipt {
            operation, conversation, request_fingerprint: format!("capture-sha256:{:?}",image.sha256()),
            adapter_fingerprint: format!("{:?}",runtime.identity()),
            source: SourceObservation {document,page,session:format!("{:?}",runtime.identity()),
                visit:format!("development-capture:{}",setup.candidate_sha256),
                content_revision:None,order_revision:None,
                capability_revision:setup.candidate_sha256.clone(),
                evidence_procedure:"selected development capture; historical association only".into()},
            intended_target:Some(page),observed_document:document,observed_page:page,
            expected_order:order.clone(),observed_order:order,persisted_revision:revision,
            observed_ms:now,evidence_origin:"development tablet capture and retained opening metadata; native qualification false".into(),
            acquisition:BindingAcquisition::ExistingCurrent,
        })?;
    Ok(())
}

pub fn run(
    runtime: &mut ResidentRuntime,
    store: &Arc<Store>,
    setup: &ObserverSetup,
    input: &mut Option<RequestInput>,
    image: &HistoricalCapture,
    root: &Path,
    overall: Instant,
) -> Result<()> {
    let name = root
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("rsdk-bootstrap-"))
        .context("composer stage nonce absent")?;
    let mut permit = ObserverPermit::new(
        name,
        &setup.candidate_sha256,
        runtime.identity(),
        image.after(),
        overall,
    )?;
    let mut ownership = TrialInput {
        quiet: input.take(),
        typing: None,
        canceled: false,
    };
    let result = execute(
        runtime,
        store,
        setup,
        &mut ownership,
        image,
        root,
        overall,
        &mut permit,
    );
    // Preserve refusal/unknown before the independent actor readmission/shutdown.
    let retained = retain(
        root,
        "trial-result.json",
        serde_json::json!({"result":format!("{result:?}"),
        "native_qualification":false,"product_submit_selected":false}),
    );
    let end = overall.min(Instant::now() + Duration::from_secs(5));
    let finished = ownership.finish_typing().and_then(|()| {
        permit.finish(end, &mut || {
            setup.current()?;
            setup.services().pulse()?;
            ownership.finish_typing()?;
            runtime.pulse()
        })
    });
    let teardown_retained = retain(
        root,
        "input-teardown.json",
        serde_json::json!({"result":format!("{finished:?}")}),
    );
    if let Err(error) = result {
        bail!(
            "composer trial failed: {error:#}; retention: {retained:?}; actor finish: {finished:?}; teardown retention: {teardown_retained:?}"
        );
    }
    retained?;
    teardown_retained?;
    finished
}

#[allow(clippy::too_many_arguments)]
fn execute(
    runtime: &mut ResidentRuntime,
    store: &Arc<Store>,
    setup: &ObserverSetup,
    input: &mut TrialInput,
    image: &HistoricalCapture,
    root: &Path,
    overall: Instant,
    permit: &mut ObserverPermit,
) -> Result<()> {
    let mut capture = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("before.png"))?;
    capture.write_all(image.png())?;
    capture.sync_all()?;
    File::open(root)?.sync_all()?;
    let conversation = Uuid::new_v4();
    let ledger = Ledger::new(store.clone());
    bind(&ledger, conversation, setup, image, runtime)?;
    let mut trial = Trial {
        setup,
        root,
        store,
        conversation,
        overall,
        input,
    };
    trial.check(overall)?;
    ensure!(
        overall.saturating_duration_since(Instant::now()) >= Duration::from_secs(100),
        "insufficient original trial time for finite seed/idle/composer phases"
    );
    let admission = overall.min(Instant::now() + Duration::from_secs(10));
    permit.admit(admission, &mut || {
        trial.check(admission)?;
        runtime.pulse()
    })?;
    retain(
        root,
        "trial.json",
        serde_json::json!({"schema":1,"conversation":conversation.to_string(),
        "candidate_sha256":setup.candidate_sha256,"text":TEXT,"native_selection_qualified":false,
        "instructions":"On composer.seed-ready, type seed then Return on the disposable selected page and release all input; copy its exact message to composer.seed-complete. Keep this input focused. On typing-ready.json, put the caret between the two appended empty paragraphs, type exactly composer trial, press Return once, then stop typing. A Reader/Undo/Redo gesture or composer-store/cancel aborts.",
        "owned_persistent_path":root,"product_submit_selected":false,"effects":"one provision and at most one conditional clear; no replay"}),
    )?;
    let handoff = overall.min(Instant::now() + Duration::from_secs(5));
    permit.begin_typing(handoff, &mut || {
        trial.check(handoff)?;
        runtime.pulse()
    })?;
    trial.allow_typing()?;
    let seed_deadline = overall.min(Instant::now() + Duration::from_secs(20));
    permit.seed_ready(seed_deadline, &mut || {
        trial.check(seed_deadline)?;
        runtime.pulse()
    })?;
    trial.require_quiet_again()?;
    // Fresh joined page facts and pixels after ordinary developer typing. The
    // marker above is not a typed-text proof; visual seed review stays explicit.
    trial.capture(runtime, "after-seed", image.after())?;
    let state = match trial.call(
        runtime,
        "provision",
        fresh(),
        Call::Provision(image.after().clone()),
    )? {
        Reply::Composer(wire::Reply::Provisioned(wire::Outcome::Verified { value, .. })) => value,
        other => bail!("provision did not verify: {other:?}"),
    };
    let empty = trial.read(runtime, "initial-observe", &state.identity)?;
    ensure!(empty.units.is_empty(), "fresh composer is not empty");
    // No-input expiry deliberately closes only transport. It does not prove
    // no-send/retirement, and never authorizes effect replay. A fresh observed
    // identical range is the next positive read result, or the trial ends.
    let idle_started = Instant::now();
    let idle = trial.call(
        runtime,
        "idle-await",
        fresh(),
        Call::Read(state.identity.clone(), Some(vec![RETURN, ENTER])),
    );
    retain(
        root,
        "idle-timing.json",
        serde_json::json!({"elapsed_ms":idle_started.elapsed().as_millis(),
        "result":format!("{idle:?}"),"expected_native_budget_ms":20000}),
    )?;
    ensure!(
        idle.is_err(),
        "idle phase unexpectedly received a key/reply"
    );
    ensure!(
        idle_started.elapsed() >= Duration::from_secs(19),
        "idle failed before intended expiry window"
    );
    let lost = runtime.retire_connection();
    retain(
        root,
        "idle-loss.json",
        serde_json::json!({"loss":format!("{lost:?}"),"no_send_inferred":false}),
    )?;
    ensure!(
        lost == LostRequest::NoPendingMutation,
        "idle left unresolved effect"
    );
    let deadline = overall.min(Instant::now() + Duration::from_secs(10));
    runtime.reconnect(deadline, &mut || trial.check(deadline))?;
    let resumed = trial.read(runtime, "idle-reconnected-observe", &state.identity)?;
    ensure!(resumed.units.is_empty(), "idle composer changed");
    // Overlap descriptors before deliberately leaving quiet admission. This
    // local policy is never used by ordinary observer/capture/effect probes.
    let key = match trial.call(
        runtime,
        "typing-key",
        fresh(),
        Call::AwaitDiagnosed(state.identity.clone(), vec![RETURN, ENTER]),
    )? {
        Reply::Composer(wire::Reply::KeyObserved(wire::Outcome::Verified { value, .. })) => value,
        other => bail!("diagnostic key did not verify: {other:?}"),
    };
    ensure!(
        !key.key.auto_repeat && key.key.count == 1 && key.key.modifiers == 0,
        "diagnostic key was repeated/modified"
    );
    ensure!(
        key.draft.state.identity == state.identity
            && key.draft.units == TEXT.encode_utf16().collect::<Vec<_>>(),
        "diagnostic key did not capture exactly the original same-range trial text"
    );
    // Never clear from the historical key snapshot. Observe the actual stock
    // newline with new operation IDs and a finite polling bound, no +1 rule.
    let expected: Vec<_> = format!("{TEXT}\n").encode_utf16().collect();
    let settle = overall.min(Instant::now() + Duration::from_secs(5));
    let mut draft = None;
    for index in 0..12 {
        trial.check(settle)?;
        let current = trial.read_until(
            runtime,
            &format!("post-key-{index}"),
            &state.identity,
            settle,
        )?;
        ensure!(
            current.state.identity == key.draft.state.identity,
            "post-key native range changed"
        );
        if current.units == expected && current.state.revision != key.draft.state.revision {
            draft = Some(current);
            break;
        }
        std::thread::sleep(Duration::from_millis(100)); // pacing, never success evidence
    }
    let draft = draft.context("exact synthetic text plus ordinary newline not observed")?;
    let now = millis()?;
    let exchange = Uuid::new_v4();
    let user = Turn {
        id: remarkable_reader_buddy::writer::stage(exchange, "composer-user"),
        conversation,
        exchange,
        sequence: 0,
        role: Role::User,
        mode: Mode::Writer,
        outcome: Outcome::Interpreted,
        text: Some(draft.text()?),
        sources: vec![],
        correction_of: None,
        created_ms: now,
        updated_ms: now,
        completion: None,
    };
    let retained = RetainedUser::append_user(
        &ledger,
        ledger.expected(Namespace::Conversation, conversation)?,
        user,
        draft,
    )?;
    retain(
        root,
        "durable-user.json",
        serde_json::json!({"conversation":conversation.to_string(),
        "clear_operation":retained.operation().0.to_string(),"draft":format!("{:?}",retained.draft()),
        "records":ledger.inspect(conversation,false)?}),
    )?;
    let clear = trial.call(
        runtime,
        "clear",
        retained.operation(),
        Call::Clear(&retained),
    )?;
    ensure!(
        matches!(
            clear,
            Reply::Composer(wire::Reply::Cleared(wire::Outcome::Verified { .. }))
        ),
        "clear did not verify: {clear:?}"
    );
    let after = trial.read(runtime, "after-clear", &state.identity)?;
    ensure!(after.units.is_empty(), "cleared range is not empty");
    trial.capture(runtime, "after-clear-visual", image.after())?;
    retain(
        root,
        "complete.json",
        serde_json::json!({"observed_empty":true,"durable_user_retained":true,
        "native_render_or_save_qualified":false,"seed_visual_review_required":true,"product_submit_selected":false}),
    )?;
    Ok(())
}
