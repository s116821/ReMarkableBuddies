//! Development-only two-party input boundary. No SDK operation or UI setup is
//! performed here. The caller retains runtime/service checks and recovery.
use super::*;
use remarkable_open_sdk::{native_adapter::RuntimeIdentity, PageKey, Uuid};

const POLL: Duration = Duration::from_millis(20);
const NAMES: [&str; 6] = [
    "observer-ended",
    "actor-ended",
    "ready",
    "complete",
    "observer-admitted",
    "actor-admitted",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePlan {
    pub document: String,
    pub page: String,
    pub order: Vec<String>,
}
impl CapturePlan {
    pub fn facts(&self) -> Result<remarkable_open_sdk::native_adapter::ReaderFacts> {
        fn id(value: &str) -> Result<Uuid> {
            let id = Uuid::parse(value).context("invalid fixture UUID")?;
            ensure!(id.to_string() == value, "noncanonical fixture UUID");
            Ok(id)
        }
        ensure!((1..=17).contains(&self.order.len()), "fixture order bound");
        remarkable_open_sdk::native_adapter::ReaderFacts::new(
            PageKey {
                document: id(&self.document)?,
                page: id(&self.page)?,
            },
            self.order.iter().map(|v| id(v)).collect::<Result<_>>()?,
        )
        .map_err(|error| anyhow::anyhow!("invalid fixture facts: {error:?}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    nonce: String,
    candidate_sha256: String,
    observer: ProcessIdentity,
    runtime: ProcessIdentity,
    boot: String,
    activation: String,
    profile_sha256: String,
    executable_sha256: String,
    adapter_sha256: String,
}
impl Binding {
    pub(super) fn observer_pid(&self) -> u32 {
        self.observer.pid
    }
    pub fn new(name: &str, candidate: &str, runtime: &RuntimeIdentity) -> Result<Self> {
        let hex = |bytes: &[u8]| bytes.iter().map(|v| format!("{v:02x}")).collect::<String>();
        let value = Self {
            nonce: name.into(),
            candidate_sha256: candidate.into(),
            observer: ProcessIdentity::current()?,
            runtime: ProcessIdentity {
                pid: runtime.pid,
                start_ticks: runtime.start_ticks,
            },
            boot: runtime.boot.to_string(),
            activation: runtime.activation.to_string(),
            profile_sha256: hex(&runtime.profile_sha256),
            executable_sha256: hex(&runtime.executable_sha256),
            adapter_sha256: hex(&runtime.adapter_sha256),
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        nonce(&self.nonce)?;
        for value in [
            &self.candidate_sha256,
            &self.profile_sha256,
            &self.executable_sha256,
            &self.adapter_sha256,
        ] {
            digest(value)?;
        }
        for value in [&self.boot, &self.activation] {
            ensure!(
                Uuid::parse(value)
                    .context("invalid setup UUID")?
                    .to_string()
                    == *value,
                "invalid setup UUID"
            );
        }
        self.observer.validate()?;
        self.runtime.validate()?;
        ensure!(self.observer != self.runtime, "setup processes must differ");
        Ok(())
    }
    pub(super) fn current(&self, context: &Context, observer: u32) -> Result<()> {
        self.validate()?;
        ensure!(
            self.nonce == context.nonce
                && self.candidate_sha256 == context.digest
                && self.boot == context.candidate.boot
                && self.observer.pid == observer,
            "setup candidate/process mismatch"
        );
        ensure!(
            ProcessIdentity::read(observer)? == self.observer
                && ProcessIdentity::read(self.runtime.pid)? == self.runtime,
            "setup process changed"
        );
        // Both processes must remain inside this exact PID1-owned actor group.
        for expected in [&self.observer, &self.runtime] {
            let group = GroupBaseline::capture_process(
                Path::new(MOUNT),
                &owned(&context.nonce, "actor"),
                expected.pid,
            )?;
            ensure!(group.parent() == expected, "setup group/process mismatch");
        }
        Ok(())
    }
}

pub struct ObserverSetup {
    pub plan: CapturePlan,
    pub candidate_sha256: String,
    pub mailbox: Mailbox,
    candidate: Pinned,
}
impl ObserverSetup {
    pub fn load(name: &str, deadline: Instant) -> Result<Option<Self>> {
        nonce(name)?;
        let (pin, bytes) = read_pin(&stage(name).join("candidate.json"), deadline)?;
        let candidate: Candidate = serde_json::from_slice(&bytes)?;
        ensure!(
            candidate.nonce == name
                && candidate.kind == Kind::Native
                && candidate.fault == Fault::None,
            "observer candidate mismatch"
        );
        if candidate.version == 1 && candidate.capture.is_none() {
            return Ok(None);
        }
        ensure!(candidate.version == 2, "unsupported capture candidate");
        let plan = candidate.capture.context("missing capture plan")?;
        plan.facts()?;
        verify_process(
            &ProcessIdentity::current()?,
            stage(name)
                .join("resident_bootstrap_observer")
                .to_str()
                .unwrap(),
            digest(&candidate.observer_sha256)?,
            &std::env::args()
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            deadline,
        )?;
        let mailbox = Mailbox::open(&root(name))?;
        mailbox.empty()?;
        Ok(Some(Self {
            plan,
            candidate_sha256: sha(&bytes),
            mailbox,
            candidate: pin,
        }))
    }
    pub fn current(&self) -> Result<()> {
        self.candidate.current()?;
        self.mailbox.current()
    }
    pub fn admit(
        &mut self,
        binding: &Binding,
        input: &mut Option<RequestInput>,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let Self {
            mailbox, candidate, ..
        } = self;
        observer(
            mailbox,
            binding,
            input,
            || RequestInput::new(TriggerCorner::LowerLeft, None),
            deadline,
            &mut || {
                candidate.current()?;
                progress()
            },
        )
    }
}

/// Retain through the actual capture. Every consumed/published record is pinned;
/// existence of a partially written JSON file never signals phase completion.
pub struct Mailbox {
    root: PathBuf,
    root_identity: (u64, u64),
    pins: Vec<Pinned>,
    consumed: Vec<String>,
}
impl Mailbox {
    pub fn open(root: &Path) -> Result<Self> {
        let meta = private_root(root)?.metadata()?;
        Ok(Self {
            root: root.into(),
            root_identity: (meta.dev(), meta.ino()),
            pins: Vec::new(),
            consumed: Vec::new(),
        })
    }
    pub fn current(&self) -> Result<()> {
        let meta = private_root(&self.root)?.metadata()?;
        ensure!(
            (meta.dev(), meta.ino()) == self.root_identity,
            "setup root replaced"
        );
        for pin in &self.pins {
            pin.current()?;
        }
        Ok(())
    }
    fn leaf(name: &str) -> Result<String> {
        ensure!(NAMES.contains(&name), "invalid setup phase");
        Ok(format!("setup.{name}.json"))
    }
    pub fn empty(&self) -> Result<()> {
        self.absent(&NAMES)
    }
    fn absent(&self, names: &[&str]) -> Result<()> {
        self.current()?;
        for name in names {
            let leaf = Self::leaf(name)?;
            ensure!(
                !present(&self.root.join(&leaf))?
                    && !present(&self.root.join(format!("{leaf}.complete")))?,
                "preexisting setup record"
            );
        }
        Ok(())
    }
    pub fn publish(&mut self, name: &str, binding: &Binding, deadline: Instant) -> Result<()> {
        check(deadline)?;
        binding.validate()?;
        self.current()?;
        let leaf = Self::leaf(name)?;
        ensure!(
            !present(&self.root.join(format!("{leaf}.complete")))?,
            "preexisting setup completion"
        );
        write_new(&self.root, &leaf, &serde_json::to_vec(binding)?)?;
        write_new(&self.root, &format!("{leaf}.complete"), b"")?;
        ensure!(
            self.receive(name, deadline)?.as_ref() == Some(binding),
            "published setup record changed"
        );
        Ok(())
    }
    pub fn receive(&mut self, name: &str, deadline: Instant) -> Result<Option<Binding>> {
        check(deadline)?;
        self.current()?;
        ensure!(
            !self.consumed.iter().any(|v| v == name),
            "setup record already consumed"
        );
        let leaf = Self::leaf(name)?;
        let complete = self.root.join(format!("{leaf}.complete"));
        if !present(&complete)? {
            return Ok(None);
        }
        let (complete_pin, marker) = read_pin(&complete, deadline)?;
        ensure!(marker.is_empty(), "invalid setup completion marker");
        let (pin, bytes) = read_pin(&self.root.join(leaf), deadline)?;
        ensure!(bytes.len() <= 4096, "setup record too large");
        let value: Binding = serde_json::from_slice(&bytes)?;
        value.validate()?;
        complete_pin.current()?;
        pin.current()?;
        self.pins.extend([complete_pin, pin]);
        self.consumed.push(name.into());
        Ok(Some(value))
    }
    pub fn wait(
        &mut self,
        name: &str,
        binding: &Binding,
        deadline: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        loop {
            check(deadline)?;
            progress()?;
            if let Some(value) = self.receive(name, deadline)? {
                ensure!(&value == binding, "setup binding changed");
                progress()?;
                self.current()?;
                return check(deadline);
            }
            thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

/// Distinct guards are consumed at the explicit boundary, never reset.
pub trait InputGuard {
    fn quiet(&mut self) -> Result<()>;
}
impl InputGuard for RequestInput {
    fn quiet(&mut self) -> Result<()> {
        ensure!(
            self.poll()? == InputProgress::Quiet,
            "setup/capture canceled by input"
        );
        Ok(())
    }
}
fn quiet<I: InputGuard>(input: &mut Option<I>) -> Result<()> {
    input.as_mut().context("missing setup input guard")?.quiet()
}

/// Called only after the resident is launched and verified. The caller must
/// retain this mailbox and the returned fresh guard through its one capture.
pub fn observer<I: InputGuard>(
    mailbox: &mut Mailbox,
    binding: &Binding,
    input: &mut Option<I>,
    acquire: impl FnOnce() -> Result<I>,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    check(deadline)?;
    progress()?;
    mailbox.empty()?;
    quiet(input)?;
    drop(input.take());
    mailbox.publish("observer-ended", binding, deadline)?;
    mailbox.wait("actor-ended", binding, deadline, progress)?;
    mailbox.absent(&["ready", "complete", "observer-admitted", "actor-admitted"])?;
    mailbox.publish("ready", binding, deadline)?;
    mailbox.wait("complete", binding, deadline, progress)?;
    *input = Some(acquire()?);
    quiet(input)?;
    mailbox.publish("observer-admitted", binding, deadline)?;
    mailbox.wait("actor-admitted", binding, deadline, &mut || {
        progress()?;
        quiet(input)
    })?;
    quiet(input)
}

/// Caller has consumed/validated observer-ended, including actual process/group
/// identity. No setup-ready exists until this participant also ends admission.
pub fn actor<I: InputGuard>(
    mailbox: &mut Mailbox,
    binding: &Binding,
    input: &mut Option<I>,
    acquire: impl FnOnce() -> Result<I>,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    check(deadline)?;
    progress()?;
    mailbox.absent(&[
        "actor-ended",
        "ready",
        "complete",
        "observer-admitted",
        "actor-admitted",
    ])?;
    quiet(input)?;
    drop(input.take());
    mailbox.publish("actor-ended", binding, deadline)?;
    mailbox.wait("complete", binding, deadline, progress)?;
    *input = Some(acquire()?);
    quiet(input)?;
    mailbox.wait("observer-admitted", binding, deadline, &mut || {
        progress()?;
        quiet(input)
    })?;
    quiet(input)?;
    // This final acknowledgement releases the observer to capture/shutdown.
    // Do not require a live resident again after publishing it: a fast capture
    // may already have completed and reaped that child normally.
    mailbox.publish("actor-admitted", binding, deadline)
}

/// The initial transport may have retired during deliberate setup. Establish
/// exactly one fresh connection to the same verified child, then one capture,
/// under the same original deadline and fresh input/context checks.
pub fn capture_after_setup(
    runtime: &mut crate::workflow::resident_runtime::ResidentRuntime,
    expected: remarkable_open_sdk::native_adapter::ReaderFacts,
    operation: remarkable_open_sdk::OperationId,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> Result<remarkable_open_sdk::native_adapter::HistoricalCapture> {
    runtime.reconnect(deadline, progress)?;
    let result = runtime.with_connection(deadline, progress, |connection, progress| {
        connection.capture_source_with_progress(operation, expected, deadline, progress)
    })?;
    match result {
        remarkable_open_sdk::native_adapter::Reply::Captured(image) => Ok(image),
        other => anyhow::bail!("capture refused: {other:?}"),
    }
}

#[cfg(test)]
mod tests;
