//! Explicit composer-only development admission. This never relaxes ordinary
//! bootstrap input/preservation and never claims native selection qualification.
use super::*;
use crate::device::{input_observer::InputObserver, interaction::Interaction};
use nix::libc;
use remarkable_open_sdk::native_adapter::{ReaderFacts, RuntimeIdentity};
use std::os::fd::AsRawFd;
mod input;
pub(super) use input::Owner as InputOwner;

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum InputMode {
    #[default]
    Manual,
    ActorFixed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionWire {
    schema: u8,
    nonce: String,
    candidate_sha256: String,
    policy: String,
    document: String,
    page: String,
    order: Vec<String>,
    rm_sha256: String,
    rm_bytes: u64,
    thumbnail_sha256: String,
    thumbnail_bytes: u64,
    #[serde(default)]
    input_mode: InputMode,
}
pub(super) struct Selection {
    wire: SelectionWire,
    pin: Pinned,
    sha256: String,
}
impl Selection {
    fn read(
        stage: &Path,
        name: &str,
        candidate: &str,
        facts: &ReaderFacts,
        deadline: Instant,
    ) -> Result<Self> {
        let path = stage.join("composer-trial.json");
        let (pin, bytes) = read_pin(&path, deadline)?;
        ensure!(
            bytes.len() <= 4096 && fs::symlink_metadata(&path)?.mode() & 0o7777 == 0o600,
            "unsafe composer sidecar"
        );
        let wire: SelectionWire = serde_json::from_slice(&bytes)?;
        ensure!(
            wire.schema == 1
                && wire.nonce == name
                && wire.candidate_sha256 == candidate
                && wire.policy == "owned-composer-mechanics-v1"
                && wire.document == facts.page().document.to_string()
                && wire.page == facts.page().page.to_string()
                && wire.order
                    == facts
                        .order()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
            "composer sidecar does not match admitted selected page"
        );
        for value in [&wire.rm_sha256, &wire.thumbnail_sha256] {
            digest(value)?;
        }
        ensure!(
            wire.rm_bytes > 0
                && wire.rm_bytes <= 16 * 1024 * 1024
                && wire.thumbnail_bytes > 0
                && wire.thumbnail_bytes <= 16 * 1024 * 1024,
            "composer baseline byte bound"
        );
        ensure!(
            !present(&stage.join("effect-probe.json"))?,
            "composer and effect probe cannot overlap"
        );
        pin.current()?;
        Ok(Self {
            wire,
            pin,
            sha256: sha(&bytes),
        })
    }
    pub(super) fn load(context: &Context, deadline: Instant) -> Result<Option<Self>> {
        if !present(&context.stage.join("composer-trial.json"))? {
            return Ok(None);
        }
        ensure!(
            context.candidate.kind == Kind::Native && context.candidate.fault == Fault::None,
            "composer requires an explicit nonfault native trial"
        );
        let plan = context
            .candidate
            .capture
            .as_ref()
            .context("composer capture absent")?;
        ensure!(
            plan.opening_metadata.is_some(),
            "composer opening metadata absent"
        );
        let selected = Self::read(
            &context.stage,
            &context.nonce,
            &context.digest,
            &plan.facts()?,
            deadline,
        )?;
        for (path, expected) in selected.paths() {
            ensure!(
                context
                    .candidate
                    .source
                    .iter()
                    .any(|entry| entry.path == path && entry.sha256.as_deref() == Some(expected)),
                "composer baseline differs from original source inventory"
            );
        }
        Ok(Some(selected))
    }
    fn paths(&self) -> [(String, &str); 2] {
        [
            (
                format!("{}/{}.rm", self.wire.document, self.wire.page),
                &self.wire.rm_sha256,
            ),
            (
                format!("{}.thumbnails/{}.png", self.wire.document, self.wire.page),
                &self.wire.thumbnail_sha256,
            ),
        ]
    }
    fn before(&self, context: &Context, deadline: Instant) -> Result<()> {
        self.pin.current()?;
        for ((path, expected), size) in self
            .paths()
            .into_iter()
            .zip([self.wire.rm_bytes, self.wire.thumbnail_bytes])
        {
            let (pin, bytes) = read_pin(
                &source_root(context.candidate.kind, &context.nonce).join(path),
                deadline,
            )?;
            ensure!(
                bytes.len() as u64 == size && sha(&bytes) == expected,
                "composer target baseline changed before provision"
            );
            pin.current()?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn preserve(
        &self,
        context: &Context,
        samples: &opening::Samples,
        policy: &opening::OpeningMetadata,
        plan: &setup::CapturePlan,
        current: &[SourceEntry],
        deadline: Instant,
    ) -> Result<()> {
        self.pin.current()?;
        // Admission is retained before any native effect; no stale sidecar alone
        // grants a new mutable file scope during recovery.
        let (pin, bytes) = read_pin(&context.root.join("composer.admitted"), deadline)?;
        let message: Message = serde_json::from_slice(&bytes)?;
        ensure!(
            message.schema == 1 && message.selection_sha256 == self.sha256,
            "composer admission differs"
        );
        let mut mailbox = setup::Mailbox::open(&context.root)?;
        ensure!(
            mailbox.receive("ready", deadline)?.as_ref() == Some(&message.binding),
            "composer admission lacks original setup binding"
        );
        let result = samples.verify_composer_edit(
            policy,
            plan,
            &self.wire.page,
            self.wire.rm_bytes,
            self.wire.thumbnail_bytes,
            &source_root(context.candidate.kind, &context.nonce),
            &context.candidate.source,
            current,
            deadline,
        );
        write_new(
            &context.root,
            &format!("composer-preservation-{}.json", uuid::Uuid::new_v4()),
            &serde_json::to_vec(
                &serde_json::json!({"page":self.wire.page,"result":format!("{result:?}"),
                "scope":"same original page rm/thumbnail plus accounted document metadata; no save acknowledgement"}),
            )?,
        )?;
        pin.current()?;
        self.pin.current()?;
        result
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    schema: u8,
    selection_sha256: String,
    binding: setup::Binding,
}

/// This seed-only envelope is not the ordinary two-party handoff message and
/// grants no native text proof. The operator must use the target monotonic clock.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SeedCue {
    schema: u8,
    phase: String,
    cue_nonce: String,
    message: Message,
    expires_monotonic_ns: u64,
}
fn monotonic_ns() -> Result<u64> {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: initialized by successful CLOCK_MONOTONIC output into sized storage.
    ensure!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, time.as_mut_ptr()) } == 0,
        "seed monotonic sample failed"
    );
    let time = unsafe { time.assume_init() };
    ensure!(
        time.tv_sec >= 0 && (0..1_000_000_000).contains(&time.tv_nsec),
        "invalid seed monotonic sample"
    );
    u64::try_from(time.tv_sec)?
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(time.tv_nsec as u64))
        .context("seed monotonic overflow")
}
fn seed_expiry(sample: u64, remaining: Duration) -> Result<u64> {
    ensure!(
        !remaining.is_zero() && remaining <= Duration::from_secs(20),
        "seed original budget expired or invalid"
    );
    sample
        .checked_add(u64::try_from(remaining.as_nanos())?)
        .context("seed expiry overflow")
}
impl SeedCue {
    fn new(message: Message, deadline: Instant) -> Result<Self> {
        // Explicit target clock FIRST, then remaining original Instant budget.
        // The later sample can only shorten the conservative expiry.
        let sample = monotonic_ns()?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("seed deadline expired")?;
        Ok(Self {
            schema: 1,
            phase: "composer-seed-v1".into(),
            cue_nonce: uuid::Uuid::new_v4().to_string(),
            message,
            expires_monotonic_ns: seed_expiry(sample, remaining)?,
        })
    }
    fn current(&self, now: u64, minimum: u64) -> Result<()> {
        let remaining = self
            .expires_monotonic_ns
            .checked_sub(now)
            .context("seed cue expired")?;
        ensure!(
            remaining >= minimum && remaining > 0 && remaining <= 20_000_000_000,
            "seed cue has insufficient or invalid budget"
        );
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= 4096, "seed cue exceeds bound");
        Ok(bytes)
    }
    fn acknowledge(&self, bytes: &[u8], now: u64) -> Result<()> {
        self.current(now, 1)?;
        ensure!(
            bytes.len() <= 4096 && bytes == self.bytes()?,
            "partial, stale or foreign seed acknowledgment"
        );
        Ok(())
    }
}
pub struct ObserverPermit {
    root: PathBuf,
    message: Message,
    selection: Selection,
    typing_requested: bool,
}
impl ObserverPermit {
    pub fn cue_binding(&self) -> Message {
        self.message.clone()
    }
    pub fn actor_fixed_input(&self) -> bool {
        self.selection.wire.input_mode == InputMode::ActorFixed
    }
    pub fn new(
        name: &str,
        candidate: &str,
        runtime: &RuntimeIdentity,
        facts: &ReaderFacts,
        deadline: Instant,
    ) -> Result<Self> {
        nonce(name)?;
        digest(candidate)?;
        let selection = Selection::read(
            &PathBuf::from(format!("/home/root/rsdk-bootstrap-{name}")),
            name,
            candidate,
            facts,
            deadline,
        )?;
        Ok(Self {
            root: PathBuf::from(format!("/run/rsdk-control-{name}")),
            message: Message {
                schema: 1,
                selection_sha256: selection.sha256.clone(),
                binding: setup::Binding::new(name, candidate, runtime)?,
            },
            selection,
            typing_requested: false,
        })
    }
    fn exchange(
        &self,
        request: &str,
        response: &str,
        deadline: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.selection.pin.current()?;
        pulse()?;
        check(deadline)?;
        write_new(&self.root, request, &serde_json::to_vec(&self.message)?)?;
        loop {
            check(deadline)?;
            pulse()?;
            self.selection.pin.current()?;
            let path = self.root.join(response);
            if present(&path)? {
                let (pin, bytes) = read_pin(&path, deadline)?;
                ensure!(
                    bytes.len() <= 4096
                        && serde_json::from_slice::<Message>(&bytes)? == self.message,
                    "foreign composer handoff"
                );
                pin.current()?;
                return Ok(());
            }
            pace(deadline);
        }
    }
    pub fn admit(&self, deadline: Instant, pulse: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        self.exchange("composer.request", "composer.admitted", deadline, pulse)
    }
    pub fn begin_typing(
        &mut self,
        deadline: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.typing_requested = true;
        self.exchange(
            "composer.typing-request",
            "composer.typing-admitted",
            deadline,
            pulse,
        )
    }
    /// Operator checks the target CLOCK_MONOTONIC expiry, then copies the whole
    /// fresh seed cue only after ordinary seed typing and release. No host-side
    /// receipt time renews this window, and acknowledgment grants no text proof.
    pub fn seed_ready(
        &self,
        deadline: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.selection.pin.current()?;
        pulse()?;
        check(deadline)?;
        let cue = SeedCue::new(self.message.clone(), deadline)?;
        write_new(&self.root, "composer.seed-staged", &cue.bytes()?)?;
        let directory = private_root(&self.root)?;
        pulse()?;
        self.selection.pin.current()?;
        check(deadline)?;
        cue.current(monotonic_ns()?, 5_000_000_000)?;
        // SAFETY: fixed NUL-terminated leaf names and a live pinned private
        // directory descriptor. linkat atomically refuses an existing ready cue.
        ensure!(
            unsafe {
                libc::linkat(
                    directory.as_raw_fd(),
                    c"composer.seed-staged".as_ptr(),
                    directory.as_raw_fd(),
                    c"composer.seed-ready".as_ptr(),
                    0,
                )
            } == 0,
            "seed cue publication failed"
        );
        directory.sync_all()?;
        loop {
            check(deadline)?;
            pulse()?;
            self.selection.pin.current()?;
            cue.current(monotonic_ns()?, 1)?;
            let path = self.root.join("composer.seed-complete");
            if present(&path)? {
                let (pin, bytes) = read_pin(&path, deadline)?;
                cue.acknowledge(&bytes, monotonic_ns()?)?;
                pin.current()?;
                check(deadline)?;
                cue.current(monotonic_ns()?, 1)?;
                return Ok(());
            }
            pace(deadline);
        }
    }
    pub fn finish(&self, deadline: Instant, pulse: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        if !self.typing_requested {
            return Ok(());
        }
        self.exchange("composer.ended", "composer.actor-ended", deadline, pulse)
    }
}

pub(super) struct ActorHandoff {
    selection: Selection,
    message: Option<Message>,
    typing: Option<InputObserver>,
    end: Option<Instant>,
    finished: bool,
    seed_attempted: bool,
    typing_attempted: bool,
}
impl ActorHandoff {
    pub(super) fn load(
        context: &Context,
        deadline: Instant,
        keyboard: &mut Option<InputOwner>,
    ) -> Result<Option<Self>> {
        let Some(selection) = Selection::load(context, deadline)? else {
            return Ok(None);
        };
        if selection.wire.input_mode == InputMode::ActorFixed {
            ensure!(keyboard.is_none(), "composer keyboard already owned");
            *keyboard = Some(InputOwner::open(&context.root, deadline)?);
        }
        Ok(Some(Self {
            selection,
            message: None,
            typing: None,
            end: None,
            finished: false,
            seed_attempted: false,
            typing_attempted: false,
        }))
    }
    fn receive(&self, context: &Context, name: &str, deadline: Instant) -> Result<Option<Message>> {
        let path = context.root.join(name);
        if !present(&path)? {
            return Ok(None);
        }
        let (pin, bytes) = read_pin(&path, deadline)?;
        ensure!(bytes.len() <= 4096, "composer handoff too large");
        let message: Message = serde_json::from_slice(&bytes)?;
        ensure!(
            message.schema == 1 && message.selection_sha256 == self.selection.sha256,
            "foreign composer plan"
        );
        pin.current()?;
        Ok(Some(message))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn poll(
        &mut self,
        context: &Context,
        observer: u32,
        input: &mut Option<RequestInput>,
        deadline: Instant,
        keyboard: Option<&mut InputOwner>,
        pulse: &mut dyn FnMut(bool) -> Result<()>,
    ) -> Result<()> {
        self.selection.pin.current()?;
        ensure!(
            !present(&context.stage.join("composer-store/cancel"))?,
            "composer trial canceled"
        );
        if self.finished {
            return quiet(input);
        }
        if self.message.is_none() {
            quiet(input)?;
            let Some(message) = self.receive(context, "composer.request", deadline)? else {
                return Ok(());
            };
            message.binding.current(context, observer)?;
            let mut mailbox = setup::Mailbox::open(&context.root)?;
            ensure!(
                mailbox.receive("ready", deadline)?.as_ref() == Some(&message.binding),
                "composer setup/runtime differs"
            );
            self.selection.before(context, deadline)?;
            write_new(
                &context.root,
                "composer.admitted",
                &serde_json::to_vec(&message)?,
            )?;
            self.message = Some(message);
        }
        let message = self.message.as_ref().unwrap();
        if let Some(end) = self.end {
            check(end)?;
            if let Some(ended) = self.receive(context, "composer.ended", deadline)? {
                ensure!(&ended == message, "foreign composer completion");
                let mut fresh = RequestInput::new(TriggerCorner::LowerLeft, None)?;
                let typing = self
                    .typing
                    .as_mut()
                    .context("composer typing observer absent")?;
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
                    "composer actor readmission lost input or queued cancellation"
                );
                *input = Some(fresh);
                drop(self.typing.take());
                self.finished = true;
                write_new(
                    &context.root,
                    "composer.actor-ended",
                    &serde_json::to_vec(message)?,
                )?;
                return Ok(());
            }
            message.binding.current(context, observer)?;
            let events = self
                .typing
                .as_mut()
                .context("composer typing observer absent")?
                .poll()?;
            ensure!(
                events
                    .iter()
                    .all(|event| *event == Interaction::Invalidated),
                "composer canceled by gesture"
            );
            if self.selection.wire.input_mode == InputMode::ActorFixed {
                let keyboard = keyboard.context("composer keyboard absent")?;
                let typing = self
                    .typing
                    .as_mut()
                    .context("composer typing observer absent")?;
                let mut admit = |full| {
                    check(end)?;
                    self.selection.pin.current()?;
                    ensure!(
                        !present(&context.stage.join("composer-store/cancel"))?,
                        "composer trial canceled"
                    );
                    let events = typing.poll()?;
                    ensure!(
                        events
                            .iter()
                            .all(|event| *event == Interaction::Invalidated)
                            && typing.quiescent()
                            && (!full || events.is_empty()),
                        "input changed across confirmed fixed-key admission"
                    );
                    message.binding.current(context, observer)?;
                    pulse(full)
                };
                input::poll(
                    context,
                    message,
                    keyboard,
                    &mut self.seed_attempted,
                    &mut self.typing_attempted,
                    end,
                    &mut admit,
                )?;
            }
            return Ok(());
        }
        quiet(input)?;
        let Some(request) = self.receive(context, "composer.typing-request", deadline)? else {
            return Ok(());
        };
        ensure!(&request == message, "foreign composer typing request");
        message.binding.current(context, observer)?;
        let mut typing = InputObserver::new(TriggerCorner::LowerLeft, None)?;
        ensure!(
            typing.poll()?.is_empty() && typing.quiescent(),
            "input before composer actor handoff"
        );
        quiet(input)?;
        drop(input.take());
        self.typing = Some(typing);
        self.end = Some(deadline.min(Instant::now() + Duration::from_secs(100)));
        write_new(
            &context.root,
            "composer.typing-admitted",
            &serde_json::to_vec(message)?,
        )
    }
}

#[cfg(test)]
mod composer_controls {
    use super::*;
    fn cue() -> SeedCue {
        let binding=serde_json::from_value(serde_json::json!({
            "nonce":"1".repeat(32),"candidate_sha256":"2".repeat(64),
            "observer":{"pid":100,"start_ticks":10},"runtime":{"pid":101,"start_ticks":11},
            "boot":"00000000-0000-0000-0000-000000000001","activation":"00000000-0000-0000-0000-000000000002",
            "profile_sha256":"3".repeat(64),"executable_sha256":"4".repeat(64),"adapter_sha256":"5".repeat(64)
        })).unwrap();
        SeedCue {
            schema: 1,
            phase: "composer-seed-v1".into(),
            cue_nonce: uuid::Uuid::new_v4().to_string(),
            message: Message {
                schema: 1,
                selection_sha256: "6".repeat(64),
                binding,
            },
            expires_monotonic_ns: 20_000_000_100,
        }
    }
    #[test]
    fn seed_expiry_uses_remaining_original_budget() {
        assert_eq!(
            seed_expiry(100, Duration::from_secs(12)).unwrap(),
            12_000_000_100
        );
        assert!(seed_expiry(100, Duration::ZERO).is_err());
        assert!(seed_expiry(100, Duration::from_secs(21)).is_err());
        assert!(seed_expiry(u64::MAX, Duration::from_nanos(1)).is_err());
        let cue = cue();
        assert!(cue.current(15_000_000_100, 5_000_000_000).is_ok());
        assert!(cue.current(15_000_000_101, 5_000_000_000).is_err());
        assert!(cue.current(cue.expires_monotonic_ns, 1).is_err());
    }
    #[test]
    fn seed_ack_requires_whole_fresh_bound_cue_before_expiry() {
        let cue = cue();
        let bytes = cue.bytes().unwrap();
        assert!(cue.acknowledge(&bytes, 1_000_000_100).is_ok());
        assert!(cue
            .acknowledge(&bytes[..bytes.len() - 1], 1_000_000_100)
            .is_err());
        let mut foreign = cue.clone();
        foreign.cue_nonce = uuid::Uuid::new_v4().to_string();
        assert!(cue
            .acknowledge(&foreign.bytes().unwrap(), 1_000_000_100)
            .is_err());
        foreign = cue.clone();
        foreign.message.selection_sha256 = "7".repeat(64);
        assert!(cue
            .acknowledge(&foreign.bytes().unwrap(), 1_000_000_100)
            .is_err());
        foreign = cue.clone();
        foreign.expires_monotonic_ns += 1;
        assert!(cue
            .acknowledge(&foreign.bytes().unwrap(), 1_000_000_100)
            .is_err());
        assert!(cue.acknowledge(&bytes, cue.expires_monotonic_ns).is_err());
        assert!(cue
            .acknowledge(&[bytes, b"\n".to_vec()].concat(), 1_000_000_100)
            .is_err());
    }
}
