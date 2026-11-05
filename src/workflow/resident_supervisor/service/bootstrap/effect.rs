//! Explicit development sidecar and one bounded own-gesture input handoff.
use super::*;
use remarkable_open_sdk::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    schema: u8,
    nonce: String,
    candidate_sha256: String,
    mode: String,
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
pub(super) struct Selection {
    wire: Wire,
    pin: Pinned,
    sha256: String,
}
impl Selection {
    pub(super) fn load(context: &Context, deadline: Instant) -> Result<Option<Self>> {
        let path = context.stage.join("effect-probe.json");
        if !present(&path)? {
            return Ok(None);
        }
        ensure!(
            context.candidate.kind == Kind::Native,
            "effect sidecar requires native development candidate"
        );
        let (pin, bytes) = read_pin(&path, deadline)?;
        ensure!(
            bytes.len() <= 4096 && fs::symlink_metadata(&path)?.mode() & 0o7777 == 0o600,
            "unsafe effect sidecar"
        );
        let wire: Wire = serde_json::from_slice(&bytes)?;
        ensure!(
            wire.schema == 1
                && wire.nonce == context.nonce
                && wire.candidate_sha256 == context.digest,
            "foreign effect sidecar"
        );
        for text in [
            &wire.conversation,
            &wire.operation,
            &wire.observe_operation,
            &wire.navigation_operation,
            &wire.target_observe_operation,
            &wire.target,
            &wire.document,
            &wire.page,
        ] {
            ensure!(
                Uuid::parse(text)
                    .context("invalid effect UUID")?
                    .to_string()
                    == *text,
                "noncanonical effect UUID"
            );
        }
        let plan = context
            .candidate
            .capture
            .as_ref()
            .context("effect requires capture plan")?;
        ensure!(
            wire.document == plan.document && wire.page == plan.page && wire.order == plan.order,
            "effect source differs from capture plan"
        );
        ensure!(
            wire.orientation == "portrait"
                && ((wire.mode == "insert"
                    && wire.navigation_policy == "logical-next-if-source-v1"
                    && !wire.order.contains(&wire.target))
                    || ((wire.mode == "select"
                        || (wire.mode == "reuse" && wire.target == wire.page))
                        && wire.navigation_policy == "none")),
            "unsupported development effect policy"
        );
        if let Some(pen) = &wire.diagnostic_pen_line {
            ensure!(
                wire.mode == "reuse"
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
            for id in ids {
                ensure!(
                    Uuid::parse(id).context("invalid pen UUID")?.to_string() == *id,
                    "noncanonical pen UUID"
                );
            }
            for (suffix, file) in [
                (
                    format!("{}/{}.rm", wire.document, wire.target),
                    &pen.baseline.rm,
                ),
                (
                    format!("{}.thumbnails/{}.png", wire.document, wire.target),
                    &pen.baseline.thumbnail,
                ),
            ] {
                digest(&file.sha256)?;
                ensure!(
                    file.bytes > 0
                        && file.bytes <= 16 * 1024 * 1024
                        && context
                            .candidate
                            .source
                            .iter()
                            .any(|entry| entry.path == suffix
                                && entry.sha256.as_deref() == Some(file.sha256.as_str())),
                    "pen baseline is not original source inventory"
                );
            }
        }
        Ok(Some(Self {
            wire,
            pin,
            sha256: sha(&bytes),
        }))
    }
    pub(super) fn insertion_target(&self) -> Option<&str> {
        (self.wire.mode == "insert").then_some(self.wire.target.as_str())
    }
    pub(super) fn reuse_pen_baseline(&self) -> Option<(&str, u64, u64)> {
        (self.wire.mode == "reuse").then_some(())?;
        let pen = self.wire.diagnostic_pen_line.as_ref()?;
        Some((
            &self.wire.target,
            pen.baseline.rm.bytes,
            pen.baseline.thumbnail.bytes,
        ))
    }
    pub(super) fn verify_before_storage(&self, context: &Context, deadline: Instant) -> Result<()> {
        let pen = self
            .wire
            .diagnostic_pen_line
            .as_ref()
            .context("reuse pen not selected")?;
        let (pin, bytes) = read_pin(
            &PathBuf::from(format!(
                "/run/rsdk-bootstrap-{}/effect-probe.pen.before-storage.json",
                context.nonce
            )),
            deadline,
        )?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            value.get("schema").and_then(serde_json::Value::as_u64) == Some(1)
                && value.get("nonce").and_then(serde_json::Value::as_str)
                    == Some(context.nonce.as_str())
                && value
                    .get("candidate_sha256")
                    .and_then(serde_json::Value::as_str)
                    == Some(context.digest.as_str())
                && value.get("plan_sha256").and_then(serde_json::Value::as_str)
                    == Some(self.sha256.as_str())
                && value.get("operation").and_then(serde_json::Value::as_str)
                    == Some(pen.operation.as_str())
                && value.get("document").and_then(serde_json::Value::as_str)
                    == Some(self.wire.document.as_str())
                && value.get("target").and_then(serde_json::Value::as_str)
                    == Some(self.wire.target.as_str())
                && value.get("phase").and_then(serde_json::Value::as_str) == Some("before-contact"),
            "foreign reuse storage baseline receipt"
        );
        for (name, file) in [
            ("rm", &pen.baseline.rm),
            ("thumbnail", &pen.baseline.thumbnail),
        ] {
            ensure!(
                value
                    .pointer(&format!("/state/{name}/sha256"))
                    .and_then(serde_json::Value::as_str)
                    == Some(file.sha256.as_str())
                    && value
                        .pointer(&format!("/state/{name}/bytes"))
                        .and_then(serde_json::Value::as_u64)
                        == Some(file.bytes),
                "reuse storage receipt differs from original backup binding"
            );
        }
        pin.current()?;
        self.current()
    }
    fn current(&self) -> Result<()> {
        self.pin.current()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationBinding {
    schema: u8,
    nonce: String,
    candidate_sha256: String,
    plan_sha256: String,
    operation: String,
    action: String,
    observer: ProcessIdentity,
}
impl NavigationBinding {
    pub fn new(nonce_value: &str, candidate: &str, plan: &str, operation: &str) -> Result<Self> {
        nonce(nonce_value)?;
        digest(candidate)?;
        digest(plan)?;
        ensure!(
            Uuid::parse(operation)
                .context("invalid navigation UUID")?
                .to_string()
                == operation,
            "noncanonical navigation UUID"
        );
        Ok(Self {
            schema: 1,
            nonce: nonce_value.into(),
            candidate_sha256: candidate.into(),
            plan_sha256: plan.into(),
            operation: operation.into(),
            action: "logical-next-v1".into(),
            observer: ProcessIdentity::current()?,
        })
    }
    pub fn pen(nonce_value: &str, candidate: &str, plan: &str, operation: &str) -> Result<Self> {
        let mut value = Self::new(nonce_value, candidate, plan, operation)?;
        value.action = "diagnostic-pen-line-v1".into();
        Ok(value)
    }
    fn phase(&self, phase: &str) -> Result<String> {
        let suffix = phase
            .strip_prefix("effect-navigation.")
            .context("invalid handoff phase")?;
        ensure!(
            ["request", "actor-ended", "released", "actor-admitted"].contains(&suffix),
            "invalid handoff phase"
        );
        Ok(format!(
            "{}.{}",
            match self.action.as_str() {
                "logical-next-v1" => "effect-navigation",
                "diagnostic-pen-line-v1" => "effect-pen",
                _ => anyhow::bail!("invalid contact action"),
            },
            suffix
        ))
    }
    fn root(&self) -> PathBuf {
        PathBuf::from(format!("/run/rsdk-control-{}", self.nonce))
    }
    fn publish(&self, phase: &str) -> Result<()> {
        write_new(
            &self.root(),
            &self.phase(phase)?,
            &serde_json::to_vec(self)?,
        )
    }
    fn receive(&self, phase: &str, deadline: Instant) -> Result<bool> {
        let path = self.root().join(self.phase(phase)?);
        if !present(&path)? {
            return Ok(false);
        }
        let (pin, bytes) = read_pin(&path, deadline)?;
        ensure!(
            bytes.len() <= 4096 && serde_json::from_slice::<Self>(&bytes)? == *self,
            "foreign navigation handoff"
        );
        pin.current()?;
        Ok(true)
    }
    fn wait(
        &self,
        phase: &str,
        deadline: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        loop {
            check(deadline)?;
            pulse()?;
            if self.receive(phase, deadline)? {
                return check(deadline);
            }
            pace(deadline);
        }
    }
    /// Caller still owns a quiet observer guard while requesting actor release.
    pub fn begin(&self, deadline: Instant, pulse: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        pulse()?;
        check(deadline)?;
        self.publish("effect-navigation.request")?;
        self.wait(
            "effect-navigation.actor-ended",
            deadline.min(Instant::now() + Duration::from_secs(10)),
            pulse,
        )
    }
    /// Caller has physically released and admitted a NEW quiet observer guard.
    pub fn released(&self, deadline: Instant, pulse: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        pulse()?;
        check(deadline)?;
        self.publish("effect-navigation.released")?;
        self.wait("effect-navigation.actor-admitted", deadline, pulse)
    }
}

pub(super) struct ActorHandoff {
    selection: Selection,
    active: Option<(NavigationBinding, Instant)>,
    consumed: std::collections::BTreeSet<String>,
}
impl ActorHandoff {
    pub(super) fn load(context: &Context, deadline: Instant) -> Result<Option<Self>> {
        Ok(Selection::load(context, deadline)?.map(|selection| Self {
            selection,
            active: None,
            consumed: std::collections::BTreeSet::new(),
        }))
    }
    pub(super) fn poll(
        &mut self,
        context: &Context,
        observer: u32,
        input: &mut Option<RequestInput>,
        deadline: Instant,
    ) -> Result<()> {
        self.selection.current()?;
        if let Some((binding, end)) = &self.active {
            check(*end)?;
            ensure!(
                ProcessIdentity::read(observer)? == binding.observer,
                "gesture observer changed"
            );
            if binding.receive("effect-navigation.released", *end)? {
                if binding.action == "diagnostic-pen-line-v1" {
                    independent_pen_release(context, &self.selection, binding, *end)?;
                }
                let mut fresh = RequestInput::new(TriggerCorner::LowerLeft, None)?;
                ensure!(
                    fresh.poll()? == InputProgress::Quiet,
                    "gesture did not leave fresh quiet released actor input"
                );
                *input = Some(fresh);
                binding.publish("effect-navigation.actor-admitted")?;
                self.consumed.insert(binding.action.clone());
                self.active = None;
            }
            return Ok(());
        }
        quiet(input)?;
        let mut pending = Vec::new();
        for (action, leaf) in [
            ("logical-next-v1", "effect-navigation.request"),
            ("diagnostic-pen-line-v1", "effect-pen.request"),
        ] {
            if !self.consumed.contains(action) && present(&context.root.join(leaf))? {
                pending.push((action, context.root.join(leaf)));
            }
        }
        ensure!(pending.len() <= 1, "overlapping development contacts");
        let Some((action, path)) = pending.pop() else {
            return Ok(());
        };
        ensure!(
            self.selection.wire.mode == "insert"
                || (self.selection.wire.mode == "reuse" && action == "diagnostic-pen-line-v1"),
            "contact not selected by development plan"
        );
        ensure!(
            action != "logical-next-v1" || !self.consumed.contains("diagnostic-pen-line-v1"),
            "navigation cannot follow the pen contact"
        );
        let expected_operation = match action {
            "logical-next-v1" => &self.selection.wire.navigation_operation,
            "diagnostic-pen-line-v1" => {
                &self
                    .selection
                    .wire
                    .diagnostic_pen_line
                    .as_ref()
                    .context("pen contact not explicitly selected")?
                    .operation
            }
            _ => anyhow::bail!("invalid development contact action"),
        };
        let (_, bytes) = read_pin(&path, deadline)?;
        ensure!(bytes.len() <= 4096, "navigation handoff too large");
        let binding: NavigationBinding = serde_json::from_slice(&bytes)?;
        ensure!(
            binding.schema == 1
                && binding.nonce == context.nonce
                && binding.candidate_sha256 == context.digest
                && binding.plan_sha256 == self.selection.sha256
                && binding.action == action
                && &binding.operation == expected_operation
                && binding.observer.pid == observer
                && ProcessIdentity::read(observer)? == binding.observer,
            "navigation handoff does not match selected plan/observer"
        );
        // Both guards were quiet before contact. This is a declared bounded own
        // gesture window, not continuous physical-input or writing authority.
        drop(input.take());
        binding.publish("effect-navigation.actor-ended")?;
        self.active = Some((
            binding,
            deadline.min(Instant::now() + Duration::from_secs(10)),
        ));
        Ok(())
    }
}

// Independent controller release after the observer's complete raw stroke and
// released snapshot. Redundant UP is never permission to clear arbitrary input.
fn independent_pen_release(
    context: &Context,
    selection: &Selection,
    binding: &NavigationBinding,
    deadline: Instant,
) -> Result<()> {
    use std::os::{
        fd::{AsFd, AsRawFd},
        unix::fs::OpenOptionsExt,
    };
    check(deadline)?;
    let runtime = PathBuf::from(format!("/run/rsdk-bootstrap-{}", context.nonce));
    let mut identity = None;
    for phase in ["before", "after"] {
        let (pin, bytes) = read_pin(
            &runtime.join(format!("effect-probe.pen.events.{phase}.json")),
            deadline,
        )?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            value.get("nonce").and_then(serde_json::Value::as_str) == Some(context.nonce.as_str())
                && value
                    .get("candidate_sha256")
                    .and_then(serde_json::Value::as_str)
                    == Some(context.digest.as_str())
                && value.get("plan_sha256").and_then(serde_json::Value::as_str)
                    == Some(selection.sha256.as_str())
                && value.get("operation").and_then(serde_json::Value::as_str)
                    == Some(binding.operation.as_str()),
            "foreign raw pen evidence"
        );
        let observed = value
            .get("writer_identity")
            .context("missing raw pen writer identity")?
            .clone();
        if phase == "before" {
            ensure!(
                value
                    .get("both_start_axes_fresh")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true),
                "raw pen start axes not fresh"
            );
            identity = Some(observed);
        } else {
            ensure!(
                identity.as_ref() == Some(&observed)
                    && value
                        .get("trace_valid")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                    && value
                        .get("final_released")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true),
                "raw pen handoff incomplete"
            );
        }
        pin.current()?;
    }
    let expected = identity.context("missing pen identity")?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK | nix::libc::O_CLOEXEC)
        .open("/dev/input/event1")?;
    let mut device = evdev::Device::from_fd(file.into())?;
    let actual = fs::metadata(format!("/proc/self/fd/{}", device.as_fd().as_raw_fd()))?;
    ensure!(
        expected.get("device").and_then(serde_json::Value::as_u64) == Some(actual.dev())
            && expected.get("inode").and_then(serde_json::Value::as_u64) == Some(actual.ino())
            && expected.get("rdev").and_then(serde_json::Value::as_u64) == Some(actual.rdev()),
        "parent pen descriptor differs from original writer"
    );
    ensure!(
        device.get_key_state()?.iter().next().is_none() && device.get_abs_state()?[24].value == 0,
        "independent parent release found unowned held input"
    );
    check(deadline)?;
    device.send_events(&[
        evdev::InputEvent::new(3, 24, 0),
        evdev::InputEvent::new(3, 25, 100),
        evdev::InputEvent::new(1, 330, 0),
        evdev::InputEvent::new(1, 320, 0),
        evdev::InputEvent::new(0, 0, 0),
    ])?;
    thread::sleep(Duration::from_millis(10));
    ensure!(
        device.get_key_state()?.iter().next().is_none() && device.get_abs_state()?[24].value == 0,
        "independent parent pen release unconfirmed"
    );
    check(deadline)?;
    write_new(
        &context.root,
        "effect-pen.parent-release",
        &serde_json::to_vec(
            &serde_json::json!({"schema":1,"nonce":context.nonce,"candidate_sha256":context.digest,"plan_sha256":selection.sha256,"operation":binding.operation,"released":true,"device":expected,"native_authority":false}),
        )?,
    )
}
