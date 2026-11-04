//! Guarded native Reader output for installed and fixture requests.
use super::*;
use crate::workflow::resident_supervisor::recovery::activation::InstalledOutput;
use crate::{
    conversation::{
        BindingAcquisition, BindingReceipt, HistoricalPageAssociationInspection, SourceObservation,
    },
    device::{
        keyboard::Keyboard,
        touch::{DiagnosticGestureEmitter, Touch, TriggerCorner},
    },
    storage::Namespace,
};
use anyhow::{ensure, Context};
use serde::Deserialize;
use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    rc::Rc,
};
pub(crate) const PROFILE: &str = "91ba3d8c200f8bfa3898ef7410595bf4ddff62f25fba3106c7f4ed442938eb5b";
pub(crate) const ADAPTER: &str = "fe52512fd08f26bc727357106a7e9e5e0bdacdd0f93c8b3a6ba1e0ee63b843fc";
const HEADER: &str = "=== Reader Buddy Answers ===\n\n";
pub(crate) const REPEAT_PROFILE: &str =
    "40b1ddfc7a85d856e8c0091f136196f57dbdef99df29886bcca0947911d68070";
pub(crate) const REPEAT_ADAPTER: &str =
    "1c7073cb698be50f05cdf78a13a31163dc3baa5a71bcbae4373261a6a2d1fa56";
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn stored(id: remarkable_open_sdk::Uuid) -> Uuid {
    Uuid::from_bytes(*id.as_bytes())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixturePage {
    page: Uuid,
    png_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FollowUp {
    nonce: Uuid,
    target: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreationFollowUp {
    nonce: Uuid,
    document: Uuid,
    source: Uuid,
    order: Vec<Uuid>,
    png_sha256: String,
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_order(order: &[Uuid], source: Uuid) -> bool {
    (1..=16).contains(&order.len())
        && order.iter().all(|id| !id.is_nil())
        && order.iter().collect::<std::collections::HashSet<_>>().len() == order.len()
        && order.contains(&source)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema: u32,
    nonce: Uuid,
    document: Uuid,
    source: Uuid,
    order: Vec<Uuid>,
    /// Explicit operator-selected disposable fixture negatives, NOT absence of binding.
    confirmed_non_buddy: Vec<FixturePage>,
    adopt_legacy: Vec<FixturePage>,
    profile_sha256: String,
    adapter_sha256: String,
    executable_sha256: String,
    policy: String,
    follow_up: Option<FollowUp>,
    creation_follow_up: Option<CreationFollowUp>,
    blank_start_conversation: Option<Uuid>,
    writer_refinement: Option<crate::writer::Request>,
    #[serde(default)]
    activate_at_startup: bool,
    #[serde(default)]
    recognize_plain_notebook: bool,
    #[serde(default)]
    recognize_original_pdf: bool,
}
struct PinnedPlan {
    path: PathBuf,
    file: File,
    bytes: Vec<u8>,
    identity: (u64, u64, i64, i64),
}
impl PinnedPlan {
    fn current(&self) -> Result<()> {
        let a = self.file.metadata()?;
        let b = std::fs::symlink_metadata(&self.path)?;
        for m in [a, b] {
            ensure!(
                m.is_file()
                    && m.uid() == 0
                    && m.gid() == 0
                    && m.mode() & 0o777 == 0o600
                    && m.nlink() == 1
                    && (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()) == self.identity,
                "development plan identity changed"
            );
        }
        use std::os::unix::fs::FileExt;
        let mut raw = vec![0; self.bytes.len()];
        self.file.read_exact_at(&mut raw, 0)?;
        ensure!(raw == self.bytes, "development plan bytes changed");
        Ok(())
    }
}
enum OutputPin {
    Fixture(PinnedPlan),
    Installed(Box<InstalledOutput>),
}

impl OutputPin {
    fn current(&self) -> Result<()> {
        match self {
            Self::Fixture(pin) => pin.current(),
            Self::Installed(pin) => pin.current(),
        }
    }

    fn evidence_origin(&self) -> &'static str {
        match self {
            Self::Fixture(_) => "development-native-guarded-v1",
            Self::Installed(_) => "installed-native-guarded-v1",
        }
    }
}

pub struct NativeOutputBackend {
    plan: Option<Plan>,
    pin: Rc<OutputPin>,
    keyboard: Rc<RefCell<Keyboard>>,
    corner: TriggerCorner,
    runtime: Option<RuntimeIdentity>,
    original: Option<ReaderFacts>,
    adoption: Option<ReaderFacts>,
    admitted: u8,
    rendered: bool,
    reuse_binding: Option<Uuid>,
}
impl NativeOutputBackend {
    pub(crate) fn startup_activation_corner(&self) -> Option<TriggerCorner> {
        self.plan
            .as_ref()
            .is_none_or(|plan| plan.activate_at_startup)
            .then(|| self.corner.clone())
    }

    pub(crate) fn installed(pin: InstalledOutput, corner: TriggerCorner) -> Result<Self> {
        pin.current()?;
        Ok(Self::new(None, OutputPin::Installed(Box::new(pin)), corner))
    }

    fn new(plan: Option<Plan>, pin: OutputPin, corner: TriggerCorner) -> Self {
        Self {
            plan,
            pin: Rc::new(pin),
            keyboard: Rc::new(RefCell::new(Keyboard::new(false, true))),
            corner,
            runtime: None,
            original: None,
            adoption: None,
            admitted: 0,
            rendered: false,
            reuse_binding: None,
        }
    }

    pub fn load(path: &Path, corner: TriggerCorner) -> Result<Self> {
        let parent = std::fs::symlink_metadata(path.parent().context("plan parent")?)?;
        ensure!(
            parent.is_dir() && parent.uid() == 0 && parent.mode() & 0o077 == 0,
            "private development plan parent required"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(path)?;
        let m = file.metadata()?;
        ensure!(m.len() > 0 && m.len() <= 16384, "development plan bound");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let plan: Plan = serde_json::from_slice(&bytes)?;
        ensure!(
            plan.schema == 1
                && !plan.nonce.is_nil()
                && !plan.document.is_nil()
                && !plan.source.is_nil(),
            "development plan identity"
        );
        let (expected_policy, profile, adapter) = match (&plan.follow_up, &plan.creation_follow_up)
        {
            (None, None) => ("one-disposable-reader-output-v1", PROFILE, ADAPTER),
            (Some(_), None) => ("two-disposable-reader-reuses-v1", PROFILE, ADAPTER),
            (None, Some(_)) => (
                "two-disposable-reader-creations-v1",
                REPEAT_PROFILE,
                REPEAT_ADAPTER,
            ),
            (Some(_), Some(_)) => anyhow::bail!("select only one follow-up policy"),
        };
        ensure!(
            plan.policy == expected_policy
                && plan.profile_sha256 == profile
                && plan.adapter_sha256 == adapter,
            "explicit development profile required"
        );
        ensure!(is_digest(&plan.executable_sha256), "executable digest");
        ensure!(valid_order(&plan.order, plan.source), "fixture order");
        ensure!(
            plan.confirmed_non_buddy.len() <= 2 && plan.adopt_legacy.len() <= 2,
            "only source/immediate successor negatives permitted"
        );
        ensure!(
            !(plan.recognize_plain_notebook || plan.recognize_original_pdf)
                || (plan.confirmed_non_buddy.is_empty()
                    && plan.creation_follow_up.is_none()
                    && plan.adopt_legacy.is_empty()),
            "native page recognition requires a plan without fixture image proofs"
        );
        let index = plan.order.iter().position(|p| *p == plan.source).unwrap();
        if let Some(conversation) = plan.blank_start_conversation {
            ensure!(
                !conversation.is_nil()
                    && plan.follow_up.is_none()
                    && plan.creation_follow_up.is_none()
                    && plan.adopt_legacy.is_empty()
                    && plan.writer_refinement.is_none(),
                "blank start requires one explicit conversation and no follow-up or adoption"
            );
        }
        if let Some(refinement) = &plan.writer_refinement {
            ensure!(
                matches!(refinement, crate::writer::Request::Refine { .. })
                    && refinement.preflight().is_ok()
                    && plan.follow_up.is_none()
                    && plan.creation_follow_up.is_none()
                    && plan.adopt_legacy.is_empty(),
                "Writer placement requires one valid refinement without follow-up or adoption"
            );
        }
        if let Some(follow_up) = &plan.follow_up {
            ensure!(
                !follow_up.nonce.is_nil() && follow_up.nonce != plan.nonce,
                "follow-up requires its own fresh request identity"
            );
            ensure!(
                follow_up.target == plan.source
                    || plan.order.get(index + 1) == Some(&follow_up.target),
                "follow-up requires the current or immediately adjacent bound page"
            );
            ensure!(
                plan.adopt_legacy.is_empty(),
                "follow-up plans only reuse existing bindings"
            );
        }
        if let Some(second) = &plan.creation_follow_up {
            ensure!(
                !second.nonce.is_nil()
                    && second.nonce != plan.nonce
                    && !second.document.is_nil()
                    && second.document != plan.document
                    && valid_order(&second.order, second.source)
                    && plan.order.last() == Some(&plan.source)
                    && second.order.last() == Some(&second.source)
                    && is_digest(&second.png_sha256)
                    && plan.confirmed_non_buddy.len() == 1
                    && plan.adopt_legacy.is_empty(),
                "creation follow-up requires two separate end-of-document fixtures"
            );
        }
        for proof in plan
            .confirmed_non_buddy
            .iter()
            .chain(plan.adopt_legacy.iter())
        {
            ensure!(
                (proof.page == plan.source || plan.order.get(index + 1) == Some(&proof.page))
                    && proof.png_sha256.len() == 64
                    && proof
                        .png_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "fixture negative scope"
            );
        }
        ensure!(
            plan.confirmed_non_buddy
                .iter()
                .map(|p| p.page)
                .collect::<std::collections::HashSet<_>>()
                .len()
                == plan.confirmed_non_buddy.len()
                && plan
                    .adopt_legacy
                    .iter()
                    .map(|p| p.page)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == plan.adopt_legacy.len(),
            "duplicate fixture proofs"
        );
        ensure!(
            !plan
                .confirmed_non_buddy
                .iter()
                .any(|p| plan.adopt_legacy.iter().any(|other| other.page == p.page)),
            "conflicting fixture proof roles"
        );
        let pin = PinnedPlan {
            path: path.to_path_buf(),
            file,
            bytes,
            identity: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
        };
        pin.current()?;
        Ok(Self::new(Some(plan), OutputPin::Fixture(pin), corner))
    }
    fn page_file(facts: &ReaderFacts) -> Result<Vec<u8>> {
        let path = PathBuf::from("/home/root/.local/share/remarkable/xochitl")
            .join(facts.page().document.to_string())
            .join(format!("{}.rm", facts.page().page));
        Self::read_native_file(&path, 16 * 1024 * 1024)
    }

    fn read_native_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
        let parent = std::fs::symlink_metadata(path.parent().unwrap())?;
        ensure!(
            parent.is_dir() && parent.uid() == 0 && parent.mode() & 0o022 == 0,
            "native document directory changed"
        );
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
            .open(path)?;
        let before = file.metadata()?;
        ensure!(
            before.is_file()
                && before.uid() == 0
                && before.gid() == 0
                && before.nlink() == 1
                && before.mode() & 0o022 == 0
                && before.len() <= limit,
            "native asset outside supported file bounds"
        );
        let identity = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mode(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        let mut raw = Vec::new();
        (&file).take(limit + 1).read_to_end(&mut raw)?;
        ensure!(
            raw.len() as u64 == before.len()
                && identity(&before) == identity(&file.metadata()?)
                && identity(&before) == identity(&std::fs::symlink_metadata(path)?),
            "native asset changed during read"
        );
        Ok(raw)
    }

    fn target_revision(facts: &ReaderFacts) -> Result<String> {
        Ok(crate::storage::digest(&Self::page_file(facts)?))
    }

    fn recognizes_ordinary_page(
        &self,
        facts: &ReaderFacts,
        pixels: &HistoricalCapture,
        blank_start_source: bool,
    ) -> Result<bool> {
        ensure!(
            pixels.before() == facts && pixels.after() == facts,
            "native capture changed"
        );
        let root = Path::new("/home/root/.local/share/remarkable/xochitl");
        let content_path = root.join(format!("{}.content", facts.page().document));
        let raw = Self::read_native_file(&content_path, 1024 * 1024)?;
        let content: serde_json::Value = serde_json::from_slice(&raw)?;
        if content["orientation"] != "portrait" || content["zoomMode"] != "bestFit" {
            return Ok(false);
        }
        let pages = content["cPages"]["pages"]
            .as_array()
            .context("native page array absent")?;
        if pages.len() != facts.order().len()
            || !pages
                .iter()
                .zip(facts.order())
                .all(|(page, id)| page["id"] == id.to_string())
        {
            return Ok(false);
        }
        let page_id = facts.page().page.to_string();
        let page = pages
            .iter()
            .find(|p| p["id"] == page_id)
            .context("native page absent")?;
        if page.get("deleted").is_some() {
            return Ok(false);
        }
        let image = image::load_from_memory(pixels.png())?.into_rgba8();
        if image.dimensions() != (1404, 1872) {
            return Ok(false);
        }
        let ordinary = match content["fileType"].as_str() {
            Some("notebook")
                if self
                    .plan
                    .as_ref()
                    .is_none_or(|plan| plan.recognize_plain_notebook) =>
            {
                if page["template"]["value"] != "Blank" || page.get("redir").is_some() {
                    return Ok(false);
                }
                // Cover both observed heading positions independently of the toolbar/footer.
                let clear = |top, bottom| {
                    (top..bottom).all(|y| {
                        (200..1240).all(|x| {
                            let [r, g, b, a] = image.get_pixel(x, y).0;
                            a == 255 && r >= 245 && g >= 245 && b >= 245
                        })
                    })
                };
                let allow_missing_ink = blank_start_source && clear(110, 1720);
                clear(110, 330) && Self::stable_ink_only(facts, allow_missing_ink)?
            }
            Some("pdf")
                if self
                    .plan
                    .as_ref()
                    .is_none_or(|plan| plan.recognize_original_pdf) =>
            {
                let original_count = content["cPages"]["original"]["value"]
                    .as_u64()
                    .context("original PDF page count absent")?;
                let Some(redirect) = page["redir"]["value"].as_u64() else {
                    return Ok(false);
                };
                let mut redirects = std::collections::HashSet::new();
                for candidate in pages
                    .iter()
                    .filter(|candidate| candidate.get("redir").is_some())
                {
                    let index = candidate["redir"]["value"]
                        .as_u64()
                        .context("invalid PDF redirect")?;
                    if index >= original_count || !redirects.insert(index) {
                        return Ok(false);
                    }
                }
                let path = root.join(format!("{}.pdf", facts.page().document));
                let pdf = Self::read_native_file(&path, 16 * 1024 * 1024)?;
                redirects.contains(&redirect)
                    && pdf.starts_with(b"%PDF-")
                    && Self::stable_ink_only(facts, true)?
                    && pdf == Self::read_native_file(&path, 16 * 1024 * 1024)?
            }
            _ => false,
        };
        Ok(ordinary && raw == Self::read_native_file(&content_path, 1024 * 1024)?)
    }

    fn stable_ink_only(facts: &ReaderFacts, allow_missing: bool) -> Result<bool> {
        let read = || match Self::page_file(facts) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error)
                if allow_missing
                    && error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        };
        let scene = read()?;
        if let Some(bytes) = &scene {
            if !crate::device::native_text::recognizes_ink_only(bytes)? {
                return Ok(false);
            }
        }
        Ok(scene == read()?)
    }
    fn bind(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        facts: &ReaderFacts,
        new: bool,
    ) -> Result<()> {
        let original = self
            .original
            .as_ref()
            .context("development original absent")?;
        if let HistoricalPageAssociationInspection::HistoricalCandidate(_) = ledger
            .inspect_page_association(
                conversation,
                stored(facts.page().document),
                stored(facts.page().page),
            )?
        {
            return Ok(());
        }
        let operation = Uuid::new_v4();
        let runtime = self.runtime.as_ref().unwrap();
        let expected_order: Vec<Uuid> = original.order().iter().copied().map(stored).collect();
        let (request_fingerprint, evidence_procedure) = match self.pin.as_ref() {
            OutputPin::Fixture(pin) => (
                crate::storage::digest(&pin.bytes),
                "development-full-admission-fresh-observe-guarded-v1",
            ),
            OutputPin::Installed(pin) => (
                crate::storage::digest(&serde_json::to_vec(&(
                    pin.digest(),
                    operation,
                    conversation,
                    original.page().document.to_string(),
                    original.page().page.to_string(),
                    facts.page().page.to_string(),
                    &expected_order,
                ))?),
                "installed-full-admission-fresh-observe-guarded-v1",
            ),
        };
        let receipt = BindingReceipt {
            operation,
            conversation,
            request_fingerprint,
            adapter_fingerprint: hex(&runtime.adapter_sha256),
            source: SourceObservation {
                document: stored(original.page().document),
                page: stored(original.page().page),
                session: format!("{}:{}:{}", runtime.boot, runtime.pid, runtime.start_ticks),
                visit: operation.to_string(),
                content_revision: None,
                order_revision: None,
                capability_revision: hex(&runtime.profile_sha256),
                evidence_procedure: evidence_procedure.into(),
            },
            intended_target: Some(stored(facts.page().page)),
            observed_document: stored(facts.page().document),
            observed_page: stored(facts.page().page),
            expected_order,
            observed_order: facts.order().iter().copied().map(stored).collect(),
            persisted_revision: Self::target_revision(facts)?,
            observed_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
                .try_into()?,
            evidence_origin: self.pin.evidence_origin().into(),
            acquisition: if new {
                BindingAcquisition::Inserted
            } else if facts.page() == original.page() {
                BindingAcquisition::ExistingCurrent
            } else {
                BindingAcquisition::ExistingAdjacent
            },
        };
        ledger.bind(
            operation,
            ledger.expected(Namespace::Conversation, conversation)?,
            ledger.expected(
                Namespace::Conversation,
                Ledger::binding_id(receipt.observed_document, receipt.observed_page),
            )?,
            receipt,
        )?;
        Ok(())
    }
}
impl NativeOutputBackend {
    pub(crate) fn accepts_blank_start(&self) -> bool {
        matches!(self.pin.as_ref(), OutputPin::Installed(_))
    }
}

impl ResidentOutputBackend for NativeOutputBackend {
    fn selected_conversation_request(&self) -> Result<Option<SelectedConversationRequest>> {
        self.pin.current()?;
        Ok(self.plan.as_ref().and_then(|plan| {
            plan.blank_start_conversation
                .map(|conversation| SelectedConversationRequest::BlankStart(Some(conversation)))
                .or_else(|| {
                    plan.writer_refinement.clone().map(|request| {
                        SelectedConversationRequest::WriterRefinement {
                            request,
                            require_current_draft: false,
                        }
                    })
                })
        }))
    }

    fn validate_new_page_text(&self, text: &str) -> Result<()> {
        self.pin.current()?;
        validate_text(&self.keyboard.borrow(), &format!("{HEADER}{text}"))
    }

    fn admit(
        &mut self,
        runtime: &RuntimeIdentity,
        association: Option<&HistoricalPageAssociation>,
        end: Instant,
    ) -> Result<()> {
        self.pin.current()?;
        ensure!(Instant::now() < end, "native output intent expired");
        ensure!(
            crate::device::native_page::verified_contract(
                crate::device::DeviceModel::detect(),
                &std::fs::read_to_string("/etc/os-release")?
            ),
            "unqualified native device"
        );
        let (Some(plan), OutputPin::Fixture(pin)) = (&self.plan, self.pin.as_ref()) else {
            let OutputPin::Installed(pin) = self.pin.as_ref() else {
                anyhow::bail!("native output selection missing");
            };
            ensure!(pin.admits(runtime), "installed native runtime pin mismatch");
            self.runtime = Some(runtime.clone());
            self.original = None;
            return Ok(());
        };
        let follow_up_nonce = plan
            .follow_up
            .as_ref()
            .map(|follow_up| follow_up.nonce)
            .or_else(|| {
                plan.creation_follow_up
                    .as_ref()
                    .map(|follow_up| follow_up.nonce)
            });
        let nonce = match (self.admitted, follow_up_nonce) {
            (0, _) => plan.nonce,
            (1, Some(nonce)) if self.rendered && self.runtime.as_ref() == Some(runtime) => nonce,
            _ => anyhow::bail!("development output intent already consumed"),
        };
        ensure!(
            plan.creation_follow_up.is_none() || association.is_none(),
            "creation fixture already has a page association"
        );
        let reuse_binding = if let Some(follow_up) = &plan.follow_up {
            let association = association.context("repeat fixture requires an existing binding")?;
            let receipt = association.receipt();
            ensure!(
                receipt.observed_document == plan.document
                    && receipt.observed_page == follow_up.target
                    && receipt.observed_order == plan.order,
                "repeat fixture binding differs from the pinned target or order"
            );
            Some(association.binding_revision())
        } else {
            None
        };
        ensure!(
            self.admitted == 0 || self.reuse_binding == reuse_binding,
            "repeat fixture binding revision changed"
        );
        ensure!(
            hex(&runtime.profile_sha256) == plan.profile_sha256
                && hex(&runtime.adapter_sha256) == plan.adapter_sha256
                && hex(&runtime.executable_sha256) == plan.executable_sha256,
            "development runtime pin mismatch"
        );
        // Older development builds used one fixed marker. Preserve it and refuse
        // its original nonce; unrelated fresh plans must have their own marker.
        match OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(pin.path.with_extension("consumed"))
        {
            Ok(mut legacy) => {
                let metadata = legacy.metadata()?;
                ensure!(
                    metadata.is_file()
                        && metadata.uid() == 0
                        && metadata.nlink() == 1
                        && metadata.len() <= 1024
                        && metadata.mode() & 0o077 == 0,
                    "unsafe historical development claim"
                );
                let mut text = String::new();
                (&mut legacy).take(1025).read_to_string(&mut text)?;
                let claimed = Uuid::parse_str(
                    text.split_whitespace()
                        .next()
                        .context("empty historical development claim")?,
                )?;
                ensure!(
                    !claimed.is_nil() && claimed != nonce,
                    "historical development plan already consumed"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let claim = pin
            .path
            .parent()
            .context("development plan parent")?
            .join(format!("plan-{nonce}.consumed"));
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&claim)?;
        writeln!(
            out,
            "{} {} {} {}",
            nonce, runtime.boot, runtime.pid, runtime.start_ticks
        )?;
        out.sync_all()?;
        File::open(claim.parent().unwrap())?.sync_all()?;
        self.admitted += 1;
        self.rendered = false;
        self.runtime = Some(runtime.clone());
        self.original = None;
        self.adoption = None;
        self.reuse_binding = reuse_binding;
        Ok(())
    }
    fn owned_keyboard(&mut self) -> Result<Option<PathBuf>> {
        self.keyboard.borrow_mut().owned_sysfs()
    }
    fn classify(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        runtime: &RuntimeIdentity,
        facts: &ReaderFacts,
        pixels: &HistoricalCapture,
        blank_start_source: bool,
        end: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<PageUse> {
        progress()?;
        self.pin.current()?;
        ensure!(
            Instant::now() < end && self.runtime.as_ref() == Some(runtime),
            "development target context changed"
        );
        // Retain real native fixture observations even when classification refuses.
        // This diagnostic archive is not a binding or page authority registry.
        if let OutputPin::Fixture(pin) = self.pin.as_ref() {
            let stem = format!("observed-{}-{}", facts.page().page, uuid::Uuid::new_v4());
            let directory = pin.path.parent().context("development plan parent")?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join(format!("{stem}.png")))?;
            file.write_all(pixels.png())?;
            file.sync_all()?;
            let mut receipt = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join(format!("{stem}.json")))?;
            serde_json::to_writer(
                &mut receipt,
                &serde_json::json!({"runtime":format!("{runtime:?}"),"facts":format!("{facts:?}"),"png_sha256":hex(&pixels.sha256()),"page_authority":false}),
            )?;
            receipt.sync_all()?;
        }
        let plan = self.plan.as_ref();
        let creation = plan
            .and_then(|plan| plan.creation_follow_up.as_ref())
            .filter(|_| self.admitted == 2);
        if self.original.is_none() {
            if let Some(plan) = plan {
                let (document, source, order) = creation
                    .map(|second| (second.document, second.source, second.order.as_slice()))
                    .unwrap_or((plan.document, plan.source, plan.order.as_slice()));
                ensure!(
                    stored(facts.page().document) == document
                        && stored(facts.page().page) == source
                        && facts
                            .order()
                            .iter()
                            .copied()
                            .map(stored)
                            .eq(order.iter().copied()),
                    "selected development source/order differs"
                );
            }
            self.original = Some(facts.clone());
        }
        let association = ledger.inspect_page_association(
            conversation,
            stored(facts.page().document),
            stored(facts.page().page),
        )?;
        match &association {
            HistoricalPageAssociationInspection::HistoricalCandidate(binding) => {
                let receipt = binding.receipt();
                if plan.is_none_or(|plan| plan.creation_follow_up.is_none())
                    && receipt.evidence_origin == self.pin.evidence_origin()
                    && receipt.adapter_fingerprint == hex(&runtime.adapter_sha256)
                    && receipt.source.capability_revision == hex(&runtime.profile_sha256)
                    && receipt.observed_order.iter().copied().eq(facts
                        .order()
                        .iter()
                        .copied()
                        .map(stored))
                {
                    return Ok(PageUse::RelevantBuddy);
                }
                return Ok(PageUse::Unresolved);
            }
            HistoricalPageAssociationInspection::Unresolved(
                crate::conversation::HistoricalPageAssociationUnresolved::OtherConversation
                | crate::conversation::HistoricalPageAssociationUnresolved::BindingConflicted
                | crate::conversation::HistoricalPageAssociationUnresolved::BindingTombstoned
                | crate::conversation::HistoricalPageAssociationUnresolved::RootConflicted
                | crate::conversation::HistoricalPageAssociationUnresolved::RootTombstoned,
            ) => return Ok(PageUse::Unresolved),
            _ => {}
        }
        let image = image::load_from_memory(pixels.png())?.resize_exact(
            768,
            1024,
            image::imageops::FilterType::Nearest,
        );
        let header = std::fs::read("/var/cache/reader-buddy/header-pattern.png")
            .ok()
            .and_then(|v| image::load_from_memory(&v).ok());
        let cached_header_matches =
            crate::workflow::Workflow::classify_answer_page(&image, header.as_ref())
                == crate::workflow::AnswerPageType::ExistingQA;
        // An explicit positive disposable-fixture proof is checked against the
        // real native capture, after the Ledger conflict/ownership checks above.
        // The historical framebuffer header cache can differ from native pixels.
        if plan.is_some_and(|plan| {
            plan.adopt_legacy.iter().any(|p| {
                p.page == stored(facts.page().page) && p.png_sha256 == hex(&pixels.sha256())
            })
        }) {
            log::info!("Explicit native legacy fixture matched; cached header match={cached_header_matches}");
            self.adoption = Some(facts.clone());
            return Ok(PageUse::LegacyBuddy);
        }
        if cached_header_matches {
            log::info!("Resident target unresolved: legacy header without selected adoption");
            return Ok(PageUse::Unresolved);
        }
        if plan.is_none_or(|plan| plan.recognize_plain_notebook || plan.recognize_original_pdf) {
            use crate::conversation::HistoricalPageAssociationUnresolved::BindingMissing;
            let ordinary = matches!(
                association,
                HistoricalPageAssociationInspection::Unresolved(BindingMissing)
            ) && self
                .recognizes_ordinary_page(facts, pixels, blank_start_source)
                .unwrap_or_else(|error| {
                    log::info!("Native page recognition refused: {error:#}");
                    false
                });
            progress()?;
            self.pin.current()?;
            if ordinary {
                log::info!("Recognized ordinary page from native storage and capture");
                return Ok(PageUse::ConfirmedNonBuddy);
            }
            return Ok(PageUse::Unresolved);
        }
        // The ONLY negative shortcut is an explicit fixture proof matched to real
        // fresh native pixels; no binding/cache/header absence proves non-Buddy.
        let page = stored(facts.page().page);
        let digest = hex(&pixels.sha256());
        let confirmed = match creation {
            Some(second) => second.source == page && second.png_sha256 == digest,
            None => plan.is_some_and(|plan| {
                plan.confirmed_non_buddy
                    .iter()
                    .any(|p| p.page == page && p.png_sha256 == digest)
            }),
        };
        if confirmed {
            return Ok(PageUse::ConfirmedNonBuddy);
        }
        log::info!("Resident target unresolved: no qualified association or exact fixture proof");
        Ok(PageUse::Unresolved)
    }
    fn record_rendered(
        &mut self,
        runtime: &RuntimeIdentity,
        facts: &ReaderFacts,
        pixels: &HistoricalCapture,
    ) -> Result<()> {
        self.pin.current()?;
        ensure!(
            self.runtime.as_ref() == Some(runtime)
                && pixels.before() == facts
                && pixels.after() == facts,
            "rendered diagnostic context changed"
        );
        let OutputPin::Fixture(pin) = self.pin.as_ref() else {
            log::info!(
                "Installed native output submitted on {} (Reader PID {}, start {})",
                facts.page().page,
                runtime.pid,
                runtime.start_ticks
            );
            return Ok(());
        };
        let stem = format!("rendered-{}-{}", facts.page().page, uuid::Uuid::new_v4());
        let directory = pin.path.parent().context("development plan parent")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(format!("{stem}.png")))?;
        file.write_all(pixels.png())?;
        file.sync_all()?;
        let mut receipt = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(format!("{stem}.json")))?;
        serde_json::to_writer(
            &mut receipt,
            &serde_json::json!({"runtime":format!("{runtime:?}"),"facts":format!("{facts:?}"),"png_sha256":hex(&pixels.sha256()),"phase":"after-render","page_authority":false,"persistence_verified":false}),
        )?;
        receipt.sync_all()?;
        log::info!(
            "Rendered native diagnostic: {} sha256={}",
            directory.join(format!("{stem}.png")).display(),
            hex(&pixels.sha256())
        );
        self.rendered = true;
        Ok(())
    }
    fn adopt(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        _: &RuntimeIdentity,
        facts: &ReaderFacts,
        _: Instant,
    ) -> Result<bool> {
        self.pin.current()?;
        ensure!(
            self.adoption.take().as_ref() == Some(facts),
            "legacy adoption not explicitly selected"
        );
        self.bind(ledger, conversation, facts, false)?;
        Ok(true)
    }
    fn navigate(
        &mut self,
        mut input: RequestInput,
        direction: LogicalDirection,
        _: &RuntimeIdentity,
        end: Instant,
        pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<RequestInput> {
        input.require_quiet()?;
        drop(input);
        struct Emit {
            end: Instant,
        }
        impl DiagnosticGestureEmitter for Emit {
            fn emit(
                &mut self,
                device: &mut evdev::Device,
                events: &[evdev::InputEvent],
            ) -> Result<()> {
                if !(events.len() == 3 && events[1].code() == 57 && events[1].value() == -1) {
                    ensure!(Instant::now() < self.end, "gesture expired");
                }
                device.send_events(events)?;
                Ok(())
            }
        }
        let plan = remarkable_open_sdk::navigation::gesture::plan_gesture(
            remarkable_open_sdk::navigation::gesture::TabletModel::Remarkable2,
            remarkable_open_sdk::navigation::gesture::Orientation::Portrait,
            direction,
        )
        .map_err(|e| anyhow::anyhow!("gesture: {e:?}"))?;
        let mut touch = Touch::diagnostic_scoped_gesture(32760, Box::new(Emit { end }))?;
        let result = (|| -> Result<()> {
            pulse()?;
            self.pin.current()?;
            touch.touch_start(plan.start())?;
            std::thread::sleep(plan.hold_duration());
            for &point in plan.movements() {
                pulse()?;
                self.pin.current()?;
                touch.goto_xy(point)?;
                std::thread::sleep(plan.movement_interval());
            }
            Ok(())
        })();
        let release = touch.touch_stop();
        match (result, release) {
            (Err(primary), Err(release)) => {
                return Err(primary.context(format!("own gesture release also failed: {release:#}")))
            }
            (Err(primary), _) => return Err(primary),
            (Ok(()), Err(release)) => return Err(release),
            _ => {}
        }
        let own = self.keyboard.borrow_mut().owned_sysfs()?;
        let mut fresh = RequestInput::new(self.corner.clone(), own.as_deref())?;
        fresh.require_quiet()?;
        pulse()?;
        Ok(fresh)
    }
    fn renderer(
        &mut self,
        ledger: &Ledger,
        conversation: Uuid,
        _: &RuntimeIdentity,
        target: &ReaderFacts,
        new: bool,
        end: Instant,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Box<dyn ResidentRenderer + '_>> {
        progress()?;
        self.pin.current()?;
        ensure!(Instant::now() < end, "renderer expired");
        self.bind(ledger, conversation, target, new)?;
        Ok(Box::new(Renderer {
            keyboard: self.keyboard.clone(),
            pin: self.pin.clone(),
            new,
        }))
    }
}
struct Renderer {
    keyboard: Rc<RefCell<Keyboard>>,
    pin: Rc<OutputPin>,
    new: bool,
}
fn validate_text(keyboard: &Keyboard, text: &str) -> Result<()> {
    ensure!(
        text.chars().count() <= 512 && text.len() <= 2048 && keyboard.supports_text(text),
        "development text/keyboard repertoire bound"
    );
    Ok(())
}

impl ResidentRenderer for Renderer {
    fn render(&mut self, text: &str, lease: &mut DevelopmentTargetLease<'_>) -> Result<()> {
        let text = if self.new {
            format!("{HEADER}{text}")
        } else {
            text.into()
        };
        validate_text(&self.keyboard.borrow(), &text)?;
        lease.step(|pulse: &mut dyn FnMut() -> Result<()>| {
            pulse()?;
            self.pin.current()?;
            let primary = self.keyboard.borrow_mut().key_cmd_body();
            let release = self.keyboard.borrow_mut().release_all();
            primary?;
            release?;
            pulse()
        })?;
        lease.step(|pulse: &mut dyn FnMut() -> Result<()>| {
            let result = (|| -> Result<()> {
                for ch in text.chars() {
                    pulse()?;
                    self.pin.current()?;
                    self.keyboard
                        .borrow_mut()
                        .string_to_keypresses(&ch.to_string())?;
                    pulse()?;
                }
                Ok(())
            })();
            let release = self.keyboard.borrow_mut().release_all();
            result?;
            release?;
            pulse()
        })
    }
}
