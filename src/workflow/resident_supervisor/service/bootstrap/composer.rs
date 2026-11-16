//! Explicit composer-only development admission. This never relaxes ordinary
//! bootstrap input/preservation and never claims native selection qualification.
use super::*;
use crate::device::{input_observer::InputObserver, interaction::Interaction};
use remarkable_open_sdk::native_adapter::{ReaderFacts, RuntimeIdentity};

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
struct Message {
    schema: u8,
    selection_sha256: String,
    binding: setup::Binding,
}
pub struct ObserverPermit {
    root: PathBuf,
    message: Message,
    selection: Selection,
    typing_requested: bool,
}
impl ObserverPermit {
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
    /// Operator copies the exact ready message only after ordinary seed typing
    /// and release. This acknowledgment grants no native text/currentness proof.
    pub fn seed_ready(
        &self,
        deadline: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.exchange(
            "composer.seed-ready",
            "composer.seed-complete",
            deadline,
            pulse,
        )
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
}
impl ActorHandoff {
    pub(super) fn load(context: &Context, deadline: Instant) -> Result<Option<Self>> {
        Ok(Selection::load(context, deadline)?.map(|selection| Self {
            selection,
            message: None,
            typing: None,
            end: None,
            finished: false,
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
    pub(super) fn poll(
        &mut self,
        context: &Context,
        observer: u32,
        input: &mut Option<RequestInput>,
        deadline: Instant,
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
                ensure!(
                    fresh.poll()? == InputProgress::Quiet,
                    "composer did not end with released input"
                );
                self.typing.take();
                *input = Some(fresh);
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
