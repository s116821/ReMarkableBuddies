//! Explicit development setup images. Never page identity or writing authority.
use super::*;
mod diagnostics;
pub use diagnostics::{Diagnostics, Phase};
use remarkable_open_sdk::{
    native_adapter::{HistoricalVisual, MAX_CAPTURE_PNG, VISUAL_PROCEDURE},
    OperationId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Before,
    After,
}
impl Stage {
    fn phase(self) -> &'static str {
        match self {
            Self::Before => "visual-before",
            Self::After => "visual-after",
        }
    }
    fn png(self) -> String {
        format!("setup.{}.png", self.phase())
    }
    fn receipt(self) -> String {
        format!("setup.{}.receipt.json", self.phase())
    }
}
pub struct Image {
    pub operation: OperationId,
    pub image: HistoricalVisual,
}
/// Owns the same Runtime across progress, reconnect and capture. The supplied
/// guard is checked through IO; captures may not renew their absolute deadline.
pub trait Transport {
    fn progress(&mut self, deadline: Instant) -> Result<()>;
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.progress(deadline)
    }
    fn boundary_guarded(
        &mut self,
        deadline: Instant,
        guard: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        guard()?;
        self.boundary(deadline)?;
        guard()
    }
    fn capture(
        &mut self,
        stage: Stage,
        deadline: Instant,
        guard: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Image>;
    fn capture_diagnosed(
        &mut self,
        stage: Stage,
        deadline: Instant,
        guard: &mut dyn FnMut() -> Result<()>,
        diagnostics: &mut Diagnostics,
    ) -> Result<Image> {
        let _ = diagnostics;
        self.capture(stage, deadline, guard)
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    schema: u8,
    stage: Stage,
    binding: Binding,
    operation: String,
    dimensions: [u32; 2],
    dpr_bits: u64,
    procedure: String,
    png_bytes: usize,
    sha256: String,
}

impl Mailbox {
    pub(super) fn absent_visual_files(&self) -> Result<()> {
        self.current()?;
        for stage in [Stage::Before, Stage::After] {
            for leaf in [stage.png(), stage.receipt()] {
                ensure!(
                    !present(&self.root.join(leaf))?,
                    "preexisting visual artifact"
                );
            }
        }
        Ok(())
    }
    // Read-only validation, not consumption or permission to dispatch a request.
    fn pin_binding(&mut self, phase: &str, binding: &Binding, deadline: Instant) -> Result<()> {
        let leaf = Self::leaf(phase)?;
        let (marker, bytes) = read_pin(&self.root.join(format!("{leaf}.complete")), deadline)?;
        ensure!(bytes.is_empty(), "invalid visual phase marker");
        let (record, bytes) = read_pin(&self.root.join(leaf), deadline)?;
        ensure!(bytes.len() <= 4096, "visual phase too large");
        let value: Binding = serde_json::from_slice(&bytes)?;
        value.validate()?;
        ensure!(&value == binding, "visual phase binding mismatch");
        self.pins.extend([marker, record]);
        self.current()?;
        check(deadline)
    }
    pub fn require_visual(
        &mut self,
        stage: Stage,
        binding: &Binding,
        deadline: Instant,
    ) -> Result<Receipt> {
        check(deadline)?;
        self.current()?;
        self.pin_binding(stage.phase(), binding, deadline)?;
        let (receipt_pin, bytes) = read_pin(&self.root.join(stage.receipt()), deadline)?;
        ensure!(bytes.len() <= 4096, "visual receipt too large");
        let receipt: Receipt = serde_json::from_slice(&bytes)?;
        ensure!(
            receipt.schema == 1
                && receipt.stage == stage
                && &receipt.binding == binding
                && receipt.procedure == VISUAL_PROCEDURE
                && receipt.png_bytes <= MAX_CAPTURE_PNG,
            "visual receipt scope mismatch"
        );
        receipt.binding.validate()?;
        let operation = Uuid::parse(&receipt.operation).context("invalid visual operation")?;
        ensure!(
            operation.to_string() == receipt.operation,
            "noncanonical visual operation"
        );
        let mut png_pin = Pinned::open(&self.root.join(stage.png()))?;
        let png = png_pin.bytes(MAX_CAPTURE_PNG as u64, deadline)?;
        ensure!(
            png.len() == receipt.png_bytes && sha(&png) == receipt.sha256,
            "visual PNG changed"
        );
        HistoricalVisual::new(
            receipt.dimensions,
            f64::from_bits(receipt.dpr_bits),
            png,
            digest(&receipt.sha256)?,
        )
        .map_err(|e| anyhow::anyhow!("invalid visual image: {e:?}"))?;
        receipt_pin.current()?;
        png_pin.current()?;
        self.pins.extend([receipt_pin, png_pin]);
        self.current()?;
        check(deadline)?;
        Ok(receipt)
    }
    pub fn require_visual_pair(&mut self, binding: &Binding, deadline: Instant) -> Result<()> {
        self.pin_binding("visual-after-request", binding, deadline)?;
        let before = self.require_visual(Stage::Before, binding, deadline)?;
        let after = self.require_visual(Stage::After, binding, deadline)?;
        ensure!(
            before.operation != after.operation,
            "visual operation reused"
        );
        self.current()?;
        check(deadline)
    }
    fn publish_visual(
        &mut self,
        stage: Stage,
        binding: &Binding,
        image: Image,
        deadline: Instant,
        admission: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        check(deadline)?;
        self.current()?;
        binding.validate()?;
        self.absent(&[stage.phase()])?;
        if stage == Stage::After {
            let before = self.require_visual(Stage::Before, binding, deadline)?;
            ensure!(
                before.operation != image.operation.0.to_string(),
                "visual operation reused"
            );
        }
        let receipt = Receipt {
            schema: 1,
            stage,
            binding: binding.clone(),
            operation: image.operation.0.to_string(),
            dimensions: image.image.dimensions(),
            dpr_bits: image.image.device_pixel_ratio().to_bits(),
            procedure: image.image.procedure().into(),
            png_bytes: image.image.png().len(),
            sha256: sha(image.image.png()),
        };
        let bytes = serde_json::to_vec(&receipt)?;
        ensure!(bytes.len() <= 4096, "visual receipt too large");
        write_new(&self.root, &stage.png(), image.image.png())?;
        write_new(&self.root, &stage.receipt(), &bytes)?;
        // Pin and verify partial outputs before a completion marker can exist.
        let mut png = Pinned::open(&self.root.join(stage.png()))?;
        ensure!(
            png.bytes(MAX_CAPTURE_PNG as u64, deadline)? == image.image.png(),
            "visual output changed"
        );
        let (record, observed) = read_pin(&self.root.join(stage.receipt()), deadline)?;
        ensure!(observed == bytes, "visual receipt changed");
        self.pins.extend([png, record]);
        self.current()?;
        check(deadline)?;
        self.publish_admitted(stage.phase(), binding, deadline, admission)?;
        self.require_visual(stage, binding, deadline)?;
        Ok(())
    }
    /// The outer fixed controller entry checks actor/runtime/service identity.
    pub fn validate_visual_after_request(
        &mut self,
        binding: &Binding,
        deadline: Instant,
    ) -> Result<()> {
        self.absent(&[
            "visual-after-request",
            "visual-after",
            "complete",
            "observer-admitted",
            "actor-admitted",
        ])?;
        for leaf in [Stage::After.png(), Stage::After.receipt()] {
            ensure!(!present(&self.root.join(leaf))?, "preexisting after-image");
        }
        self.require_visual(Stage::Before, binding, deadline)?;
        Ok(())
    }
}

fn boundary(
    transport: &mut impl Transport,
    guard: &mut dyn FnMut() -> Result<()>,
    deadline: Instant,
) -> Result<()> {
    check(deadline)?;
    guard()?;
    transport.boundary_guarded(deadline, guard)?;
    guard()?;
    check(deadline)
}
fn wait_for_phase(
    mailbox: &mut Mailbox,
    phase: &str,
    binding: &Binding,
    deadline: Instant,
    transport: &mut impl Transport,
    guard: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    mailbox.wait_admitted(phase, binding, deadline, &mut |full| {
        if full {
            boundary(transport, guard, deadline)
        } else {
            progress(transport, guard, deadline)
        }
    })
}

fn progress(
    transport: &mut impl Transport,
    guard: &mut dyn FnMut() -> Result<()>,
    deadline: Instant,
) -> Result<()> {
    check(deadline)?;
    guard()?;
    transport.progress(deadline)?;
    guard()?;
    check(deadline)
}
fn capture(
    mailbox: &mut Mailbox,
    binding: &Binding,
    stage: Stage,
    transport: &mut impl Transport,
    guard: &mut dyn FnMut() -> Result<()>,
    deadline: Instant,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    // This remains the original setup bound. Reconnect and API allocate their
    // own bounded phases below; neither may renew this outer deadline.
    let capture_deadline = deadline;
    diagnostics.step(
        Some(stage),
        Phase::CaptureProgress,
        capture_deadline,
        |_| progress(transport, guard, capture_deadline),
    )?;
    let image = diagnostics.step(Some(stage), Phase::Capture, capture_deadline, |trace| {
        transport.capture_diagnosed(
            stage,
            capture_deadline,
            &mut || {
                guard()?;
                mailbox.current()?;
                check(capture_deadline)
            },
            trace,
        )
    })?;
    let publication_deadline = deadline.min(Instant::now() + Duration::from_secs(10));
    diagnostics.step(
        Some(stage),
        Phase::CaptureProgress,
        publication_deadline,
        |_| progress(transport, guard, publication_deadline),
    )?;
    let root = mailbox.root.clone();
    diagnostics.step(
        Some(stage),
        Phase::Publication,
        publication_deadline,
        |_| {
            mailbox.publish_visual(stage, binding, image, publication_deadline, &mut || {
                boundary(transport, guard, publication_deadline)?;
                let mailbox = Mailbox::open(&root)?;
                mailbox.absent(&["complete", "observer-admitted", "actor-admitted"])?;
                if stage == Stage::Before {
                    mailbox.absent(&["ready", "visual-after-request", "visual-after"])?;
                }
                Ok(())
            })
        },
    )
}

/// The only two visual requests are separated by the developer's explicit
/// after-request. Setup completion still waits for inspected after-image evidence.
pub fn observer<I: InputGuard>(
    mailbox: &mut Mailbox,
    binding: &Binding,
    input: &mut Option<I>,
    acquire: impl FnOnce() -> Result<I>,
    deadline: Instant,
    transport: &mut impl Transport,
    guard: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let mut diagnostics = Diagnostics::new(&mailbox.root, binding)?;
    observer_diagnosed(
        mailbox,
        binding,
        input,
        acquire,
        deadline,
        transport,
        guard,
        &mut diagnostics,
    )
}

#[allow(clippy::too_many_arguments)]
fn observer_diagnosed<I: InputGuard>(
    mailbox: &mut Mailbox,
    binding: &Binding,
    input: &mut Option<I>,
    acquire: impl FnOnce() -> Result<I>,
    deadline: Instant,
    transport: &mut impl Transport,
    guard: &mut dyn FnMut() -> Result<()>,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    diagnostics.step(None, Phase::SetupProgress, deadline, |_| {
        progress(transport, guard, deadline)
    })?;
    boundary(transport, guard, deadline)?;
    mailbox.empty()?;
    quiet(input)?;
    drop(input.take());
    mailbox.publish("observer-ended", binding, deadline)?;
    diagnostics.step(None, Phase::ActorEnded, deadline, |_| {
        wait_for_phase(mailbox, "actor-ended", binding, deadline, transport, guard)
    })?;
    mailbox.absent(&[
        "ready",
        "complete",
        "observer-admitted",
        "actor-admitted",
        "visual-after-request",
        "visual-before",
        "visual-after",
    ])?;
    capture(
        mailbox,
        binding,
        Stage::Before,
        transport,
        guard,
        deadline,
        diagnostics,
    )?;
    let root = mailbox.root.clone();
    mailbox.publish_admitted("ready", binding, deadline, &mut || {
        boundary(transport, guard, deadline)?;
        Mailbox::open(&root)?.absent(&[
            "complete",
            "observer-admitted",
            "actor-admitted",
            "visual-after-request",
            "visual-after",
        ])
    })?;
    wait_for_phase(
        mailbox,
        "visual-after-request",
        binding,
        deadline,
        transport,
        guard,
    )?;
    mailbox.absent(&[
        "complete",
        "observer-admitted",
        "actor-admitted",
        "visual-after",
    ])?;
    capture(
        mailbox,
        binding,
        Stage::After,
        transport,
        guard,
        deadline,
        diagnostics,
    )?;
    wait_for_phase(mailbox, "complete", binding, deadline, transport, guard)?;
    mailbox.require_visual_pair(binding, deadline)?;
    progress(transport, guard, deadline)?;
    *input = Some(acquire()?);
    quiet(input)?;
    mailbox.publish_admitted("observer-admitted", binding, deadline, &mut || {
        boundary(transport, guard, deadline)?;
        quiet(input)
    })?;
    wait_for_phase(
        mailbox,
        "actor-admitted",
        binding,
        deadline,
        transport,
        &mut || {
            guard()?;
            quiet(input)
        },
    )?;
    quiet(input)
}

impl ObserverSetup {
    pub fn admit_visual(
        &mut self,
        binding: &Binding,
        input: &mut Option<RequestInput>,
        deadline: Instant,
        transport: &mut impl Transport,
    ) -> Result<()> {
        ensure!(
            self.plan.visual_setup == Some(1),
            "visual setup not selected"
        );
        let mut diagnostics = Diagnostics::new(&self.mailbox.root, binding)?;
        self.admit_visual_diagnosed(binding, input, deadline, transport, &mut diagnostics)
    }
    pub fn admit_visual_diagnosed(
        &mut self,
        binding: &Binding,
        input: &mut Option<RequestInput>,
        deadline: Instant,
        transport: &mut impl Transport,
        diagnostics: &mut Diagnostics,
    ) -> Result<()> {
        ensure!(
            self.plan.visual_setup == Some(1),
            "visual setup not selected"
        );
        let Self {
            mailbox, candidate, ..
        } = self;
        observer_diagnosed(
            mailbox,
            binding,
            input,
            || RequestInput::new(TriggerCorner::LowerLeft, None),
            deadline,
            transport,
            &mut || candidate.current(),
            diagnostics,
        )
    }
}

/// One explicit same-child reconnect and one read-only image request. The
/// caller supplies candidate/service admission; the Runtime adds child checks.
struct ProgressAdmission<'a> {
    progress: &'a mut dyn FnMut() -> Result<()>,
}
impl crate::workflow::resident_runtime::ReadOnlyAdmission for ProgressAdmission<'_> {
    fn pulse(&mut self, deadline: Instant) -> Result<()> {
        check(deadline)?;
        (self.progress)()?;
        check(deadline)
    }
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.pulse(deadline)
    }
}
struct TracedAdmission<'a> {
    inner: &'a mut dyn crate::workflow::resident_runtime::ReadOnlyAdmission,
    diagnostics: &'a mut Diagnostics,
    stage: Stage,
}
impl crate::workflow::resident_runtime::ReadOnlyAdmission for TracedAdmission<'_> {
    fn pulse(&mut self, deadline: Instant) -> Result<()> {
        self.inner.pulse(deadline)
    }
    fn boundary(&mut self, deadline: Instant) -> Result<()> {
        self.inner.boundary(deadline)
    }
    fn boundary_with_pulse(
        &mut self,
        deadline: Instant,
        runtime_pulse: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.inner.boundary_with_pulse(deadline, runtime_pulse)
    }
    fn before_api(&mut self, deadline: Instant) -> Result<()> {
        self.diagnostics.api_enter(self.stage, deadline)
    }
    fn after_api(
        &mut self,
        deadline: Instant,
        invoked: bool,
        result: &Result<remarkable_open_sdk::native_adapter::Reply>,
    ) -> Result<()> {
        self.diagnostics
            .api_result(self.stage, deadline, invoked, result)
    }
}
pub fn capture_on_runtime(
    runtime: &mut crate::workflow::resident_runtime::ResidentRuntime,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
) -> Result<Image> {
    capture_on_runtime_admitted(runtime, deadline, &mut ProgressAdmission { progress })
}
pub fn capture_on_runtime_admitted(
    runtime: &mut crate::workflow::resident_runtime::ResidentRuntime,
    deadline: Instant,
    admission: &mut dyn crate::workflow::resident_runtime::ReadOnlyAdmission,
) -> Result<Image> {
    let reconnect_deadline = deadline.min(Instant::now() + Duration::from_secs(30));
    runtime.reconnect_read_only(reconnect_deadline, admission)?;
    capture_read_only(runtime, deadline, admission)
}
fn capture_read_only(
    runtime: &mut crate::workflow::resident_runtime::ResidentRuntime,
    deadline: Instant,
    admission: &mut dyn crate::workflow::resident_runtime::ReadOnlyAdmission,
) -> Result<Image> {
    let operation =
        OperationId(Uuid::parse(&uuid::Uuid::new_v4().to_string()).expect("fresh UUID"));
    match runtime.request_read_only_capture(
        operation,
        crate::workflow::resident_runtime::ReadOnlyRequest::CaptureVisual,
        deadline,
        admission,
    )? {
        remarkable_open_sdk::native_adapter::Reply::VisualCaptured(image) => {
            Ok(Image { operation, image })
        }
        other => anyhow::bail!("visual capture refused: {other:?}"),
    }
}
pub fn capture_on_runtime_diagnosed(
    runtime: &mut crate::workflow::resident_runtime::ResidentRuntime,
    stage: Stage,
    deadline: Instant,
    progress: &mut dyn FnMut() -> Result<()>,
    diagnostics: &mut Diagnostics,
) -> Result<Image> {
    capture_on_runtime_diagnosed_admitted(
        runtime,
        stage,
        deadline,
        &mut ProgressAdmission { progress },
        diagnostics,
    )
}
pub fn capture_on_runtime_diagnosed_admitted(
    runtime: &mut crate::workflow::resident_runtime::ResidentRuntime,
    stage: Stage,
    deadline: Instant,
    admission: &mut dyn crate::workflow::resident_runtime::ReadOnlyAdmission,
    diagnostics: &mut Diagnostics,
) -> Result<Image> {
    let reconnect_deadline = deadline.min(Instant::now() + Duration::from_secs(30));
    diagnostics.step(Some(stage), Phase::Reconnect, reconnect_deadline, |_| {
        runtime.reconnect_read_only(reconnect_deadline, admission)
    })?;
    capture_read_only(
        runtime,
        deadline,
        &mut TracedAdmission {
            inner: admission,
            diagnostics,
            stage,
        },
    )
}

#[cfg(test)]
mod tests;
