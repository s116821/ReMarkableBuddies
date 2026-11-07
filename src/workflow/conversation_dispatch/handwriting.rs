//! Typed source receiving only. Native Writer/confirmation ingress is not supplied here.
use super::*;
use crate::{
    conversation::{Record, TextScope},
    handwriting::{self, runtime},
    retrieval::notebook,
};
use std::{collections::BTreeSet, io::Write};

#[derive(Clone)]
pub(crate) struct ImageRef {
    pub source: Uuid,
    pub revision: Uuid,
    pub turn: Uuid,
    pub turn_revision: Uuid,
    pub ordinal: u32,
    pub page: u32,
}
pub(crate) struct WriterImageRequest {
    pub image: ImageRef,
    pub request: String,
    pub guidance: Option<String>,
    pub model_profile: String,
}
pub(crate) struct WriterAbstention {
    store: Arc<crate::storage::Store>,
    scope: RequestScope,
    image: ImageRef,
    sample_sha256: String,
    profile: Uuid,
}

pub(crate) struct SourceReport {
    pub(crate) abstention: Option<WriterAbstention>,
    pub result: Result<(writer::Report, Option<PendingOutput>)>,
    _capture: Option<runtime::Capture>,
    #[cfg(test)]
    pub diagnostic: &'static str,
}
impl SourceReport {
    pub(crate) fn write_to(&self, output: &mut impl Write) -> Result<()> {
        let value = match &self.result {
            Ok((report, _)) => serde_json::to_value(report)?,
            Err(_) => serde_json::json!({"status":"refused"}),
        };
        serde_json::to_writer(&mut *output, &value)?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    }
}
fn resolve(
    ledger: &Ledger,
    scope: &RequestScope,
    image: &ImageRef,
) -> Result<(
    crate::conversation::ImageUse,
    crate::conversation::Turn,
    Vec<u8>,
)> {
    ensure!(image.page > 0, "invalid image page");
    ensure!(
        ledger.expected(Namespace::Source, image.source)?.0 == BTreeSet::from([image.revision]),
        "source revision changed"
    );
    ensure!(
        ledger.expected(Namespace::Conversation, image.turn)?.0
            == BTreeSet::from([image.turn_revision]),
        "source turn revision changed"
    );
    let source = ledger
        .store()
        .value(Namespace::Source, image.source)?
        .ok_or_else(|| anyhow::anyhow!("source absent"))?;
    let Record::Source(source) = Ledger::decode(&source)? else {
        anyhow::bail!("retained source has no exact ImageUse correspondence");
    };
    let turn = ledger
        .store()
        .value(Namespace::Conversation, image.turn)?
        .ok_or_else(|| anyhow::anyhow!("turn absent"))?;
    let Record::Turn(turn) = Ledger::decode(&turn)? else {
        anyhow::bail!("source turn unavailable");
    };
    ensure!(
        source.id == image.source
            && source.ordinal == image.ordinal
            && source.turn == image.turn
            && source.conversation == scope.conversation
            && turn.id == image.turn
            && turn.conversation == scope.conversation
            && turn.role == Role::User
            && turn.sources.contains(&source.id)
            && source.observation == scope.source,
        "source image association changed"
    );
    let bytes = ledger.image(image.source)?;
    Ok((source, turn, bytes))
}
impl Dispatch<'_> {
    /// Called by an admitted typed image owner; native image ingress remains open.
    #[allow(dead_code)]
    pub(crate) fn writer_source_image<E: LLMEngine>(
        &mut self,
        ledger: &Ledger,
        engine: &mut E,
        input: &WriterImageRequest,
        admitted: &RequestScope,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> SourceReport {
        let mut report = SourceReport {
            result: Err(anyhow::anyhow!("source not admitted")),
            _capture: None,
            abstention: None,
            #[cfg(test)]
            diagnostic: "source-admission",
        };
        report.result = (|| {
            self.require_ledger(ledger)?;
            ensure!(self.admits(Mode::Writer, false), "Writer disabled");
            admitted.check(ledger, observe)?;
            ensure!(
                admitted.operation != admitted.conversation,
                "invalid Writer operation identity"
            );
            writer::validate_request(&input.request)?;
            ensure!(
                input
                    .guidance
                    .as_ref()
                    .is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0')),
                "invalid Writer guidance"
            );
            let (original_source, _, png) = resolve(ledger, admitted, &input.image)?;
            // A completed Writer request/root cannot enter a second normalization.
            ensure!(
                ledger
                    .store()
                    .value(
                        Namespace::Conversation,
                        writer::stage(admitted.operation, "user")
                    )?
                    .is_none(),
                "Writer operation already started"
            );
            let stage = writer::stage(admitted.operation, "source-normalization");
            ensure!(
                ledger
                    .store()
                    .value(Namespace::Conversation, stage)?
                    .is_none(),
                "source normalization already prepared; reconcile original stage"
            );
            ensure!(
                self.input_events.len() < 4096 && self.input_events.insert(stage),
                "source normalization already admitted"
            );
            let target = notebook::admit_target(input.image.page, png, &mut |_| {
                admitted.check(ledger, observe)
            })?;
            if let Some(runtime) = self.handwriting_runtime(ledger)? {
                let captured = runtime.capture(
                    ledger.store(),
                    &BTreeSet::from([target.sha256().to_owned()]),
                    &mut || {
                        admitted
                            .check(ledger, observe)
                            .map_err(|_| handwriting::ContextFailure::Canceled)
                    },
                );
                report._capture = Some(captured);
                ensure!(
                    report._capture.as_ref().is_none_or(|c| c.failure.is_none()),
                    "handwriting capture unavailable"
                );
            }
            // Reuse the existing durable prepared-turn/source records before inference.
            // This stage is never resumed by repeating recognition after restart.
            admitted.check(ledger, observe)?;
            let source_id = writer::stage(admitted.operation, "source-image");
            let now: u64 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
                .try_into()?;
            let sample_sha256 = original_source.image.sha256.clone();
            let copied_source = crate::conversation::ImageUse {
                id: source_id,
                turn: stage,
                purpose: "retained image for source normalization; historical derivative".into(),
                ..original_source
            };
            let prepared = ledger.append(
                writer::stage(admitted.operation, "source-normalization-admission"),
                admitted.root.clone(),
                Turn {
                    id: stage,
                    conversation: admitted.conversation,
                    exchange: admitted.operation,
                    sequence: 0,
                    role: Role::User,
                    mode: Mode::Writer,
                    outcome: Outcome::Prepared,
                    text: None,
                    sources: vec![source_id],
                    correction_of: None,
                    created_ms: now,
                    updated_ms: now,
                    completion: None,
                },
                vec![copied_source],
            )?;
            let receiving = RequestScope {
                conversation: admitted.conversation,
                operation: admitted.operation,
                root: crate::conversation::ExpectedHeads(BTreeSet::from([prepared
                    .historical
                    .applied_root_revision])),
                source: admitted.source.clone(),
            };
            receiving.check(ledger, observe)?;
            let (normalized, _) = notebook::normalize_admitted(
                admitted.source.document,
                &target,
                &input.model_profile,
                report._capture.as_ref().and_then(|c| c.context.as_deref()),
                notebook::TranscriptionPolicy::WholePage,
                engine,
                &mut |_| receiving.check(ledger, observe),
            );
            #[cfg(test)]
            {
                use notebook::{NormalizeFailure as Failure, Normalized};
                report.diagnostic = match &normalized {
                    Err(Failure::Canceled) => "normalization-canceled",
                    Err(Failure::InvalidInput) => "normalization-invalid-input",
                    Err(Failure::PromptLimit) => "normalization-prompt-limit",
                    Err(Failure::ProviderFailed) => "normalization-provider-failed",
                    Err(Failure::InvalidResponse) => "normalization-invalid-response",
                    Ok(Normalized::Unreadable) => "normalization-unreadable",
                    Ok(Normalized::Page(_)) => "normalized-source-revalidation",
                    Ok(_) => "normalization-incomplete-page",
                };
            }
            if matches!(&normalized, Ok(notebook::Normalized::Unreadable)) {
                receiving.check(ledger, observe)?;
                resolve(ledger, admitted, &input.image)?;
                if let (true, Some(profile)) = (
                    self.config.handwriting.enabled,
                    self.config.handwriting.profile,
                ) {
                    report.abstention = Some(WriterAbstention {
                        store: ledger.store().clone(),
                        scope: receiving,
                        image: input.image.clone(),
                        sample_sha256,
                        profile,
                    });
                }
                anyhow::bail!("source not a complete normalized page");
            }
            let notebook::Normalized::Page(page) =
                normalized.map_err(|_| anyhow::anyhow!("source normalization refused"))?
            else {
                anyhow::bail!("source not a complete normalized page");
            };
            receiving.check(ledger, observe)?;
            resolve(ledger, admitted, &input.image)?;
            let request = writer::Request::Generate {
                conversation: admitted.conversation,
                operation: admitted.operation,
                request: input.request.clone(),
                document: page.document,
                selection: TextScope::Page {
                    page: input.image.page,
                },
                guidance: input.guidance.clone(),
            };
            #[cfg(test)]
            {
                report.diagnostic = "normalized-writer-preflight";
            }
            request
                .preflight()
                .map_err(|_| anyhow::anyhow!("normalized Writer request refused"))?;
            #[cfg(test)]
            {
                report.diagnostic = "writer-receiving";
            }
            self.writer(ledger, engine, &request, &receiving, observe)
        })();
        report
    }
}

/// A distinct owner-supplied intent, never constructed from generic Refine/erasure.
pub(crate) struct ConfirmIntent {
    pub image: ImageRef,
    pub corrected_text: String,
    pub normalized_source: Option<(Uuid, Uuid)>,
}
pub(crate) struct PreparedConfirmation {
    #[cfg(test)]
    input: Vec<u8>,
    validated: handwriting::ValidatedInput,
    owner: handwriting::LocalCorrections,
    store: Arc<crate::storage::Store>,
    scope: RequestScope,
    image: ImageRef,
    normalized_source: Option<(Uuid, Uuid)>,
}
pub(crate) struct ConfirmationReport {
    pub report: handwriting::Report,
    #[cfg(test)]
    frozen_input: Vec<u8>,
}
impl PreparedConfirmation {
    #[cfg(test)]
    pub(crate) fn frozen_input(&self) -> &[u8] {
        &self.input
    }
    pub(crate) fn execute(
        self,
        ledger: &Ledger,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<ConfirmationReport> {
        ensure!(
            Arc::ptr_eq(&self.store, ledger.store()),
            "confirmation receiving store changed"
        );
        self.scope.check(ledger, observe)?;
        resolve(ledger, &self.scope, &self.image)?;
        if let Some((id, revision)) = self.normalized_source {
            ensure!(
                ledger.expected(Namespace::Source, id)?.0 == BTreeSet::from([revision]),
                "normalized interpretation changed"
            );
        }
        use handwriting::Provider;
        Ok(ConfirmationReport {
            report: self.owner.execute(self.validated),
            #[cfg(test)]
            frozen_input: self.input,
        })
    }
}
impl ConfirmationReport {
    #[cfg(test)]
    pub(crate) fn frozen_input(&self) -> &[u8] {
        &self.frozen_input
    }
    pub(crate) fn write_to(&self, output: &mut impl Write) -> Result<()> {
        self.report.write_to(output).map_err(anyhow::Error::from)
    }
}
impl Dispatch<'_> {
    /// Native explicit-intent attachment is unfinished; never infer it from text.
    #[allow(dead_code)]
    pub(crate) fn prepare_handwriting_confirmation(
        &mut self,
        ledger: &Ledger,
        intent: &ConfirmIntent,
        admitted: &RequestScope,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<PreparedConfirmation> {
        self.require_ledger(ledger)?;
        ensure!(
            self.config.handwriting.enabled,
            "handwriting profile disabled"
        );
        admitted.check(ledger, observe)?;
        let (_, turn, png) = resolve(ledger, admitted, &intent.image)?;
        ensure!(
            png.len() <= handwriting::MAX_SAMPLE,
            "retained sample exceeds correction bound"
        );
        notebook::static_png(&png)?;
        let original = if turn.mode == Mode::Reader {
            ensure!(
                intent.normalized_source.is_none(),
                "Reader correction source mismatch"
            );
            turn.text
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Reader interpretation absent"))?
        } else {
            let (id, revision) = intent
                .normalized_source
                .ok_or_else(|| anyhow::anyhow!("Writer source interpretation absent"))?;
            ensure!(
                ledger.expected(Namespace::Source, id)?.0 == BTreeSet::from([revision]),
                "normalized source revision changed"
            );
            let envelope = ledger
                .store()
                .value(Namespace::Source, id)?
                .ok_or_else(|| anyhow::anyhow!("normalized source absent"))?;
            let Record::NormalizedText(normalized) = Ledger::decode(&envelope)? else {
                anyhow::bail!("source has no normalized interpretation");
            };
            normalized.validate()?;
            let target = notebook::admit_target(intent.image.page, png.clone(), &mut |_| {
                admitted.check(ledger, observe)
            })?;
            ensure!(
                normalized.conversation == admitted.conversation
                    && normalized.document.document == admitted.source.document
                    && normalized.document.source_sha256 == target.source_sha256(),
                "normalized text/image correspondence unavailable"
            );
            let interpreted = ledger
                .store()
                .value(Namespace::Conversation, normalized.turn)?
                .ok_or_else(|| anyhow::anyhow!("normalized source turn absent"))?;
            let Record::Turn(interpreted) = Ledger::decode(&interpreted)? else {
                anyhow::bail!("normalized source turn unavailable");
            };
            ensure!(
                interpreted.conversation == admitted.conversation
                    && interpreted.mode == Mode::Writer
                    && interpreted.role == Role::User
                    && interpreted.sources.contains(&normalized.id)
                    && normalized.document.format == crate::retrieval::SourceFormat::Notebook,
                "normalized source interpretation association unavailable"
            );
            let units = normalized
                .document
                .units
                .iter()
                .filter(|u| u.page == Some(intent.image.page))
                .collect::<Vec<_>>();
            ensure!(units.len() == 1, "ambiguous normalized page interpretation");
            units[0].text.clone()
        };
        self.prepare_confirmed_sample(ledger, admitted, intent, Some(original), &png, observe)
    }

    /// An input owner must supply the explicit save action and the exact abstention it presented.
    #[allow(dead_code)]
    pub(crate) fn prepare_writer_enrollment(
        &mut self,
        ledger: &Ledger,
        abstention: &WriterAbstention,
        corrected_text: String,
        admitted: &RequestScope,
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<PreparedConfirmation> {
        self.require_ledger(ledger)?;
        ensure!(
            Arc::ptr_eq(&abstention.store, ledger.store())
                && admitted.conversation == abstention.scope.conversation
                && admitted.operation != abstention.scope.operation
                && admitted.root.0 == abstention.scope.root.0
                && admitted.source == abstention.scope.source
                && self.config.handwriting.enabled
                && self.config.handwriting.profile == Some(abstention.profile)
                && self.admits(Mode::Writer, false),
            "Writer abstention custody changed"
        );
        admitted.check(ledger, observe)?;
        let (source, turn, png) = resolve(ledger, admitted, &abstention.image)?;
        ensure!(
            turn.mode == Mode::Writer && source.image.sha256 == abstention.sample_sha256,
            "Writer abstention image changed"
        );
        notebook::static_png(&png)?;
        let intent = ConfirmIntent {
            image: abstention.image.clone(),
            corrected_text,
            normalized_source: None,
        };
        self.prepare_confirmed_sample(ledger, admitted, &intent, None, &png, observe)
    }

    fn prepare_confirmed_sample(
        &mut self,
        ledger: &Ledger,
        admitted: &RequestScope,
        intent: &ConfirmIntent,
        original: Option<String>,
        png: &[u8],
        observe: &mut dyn FnMut() -> Result<SourceObservation>,
    ) -> Result<PreparedConfirmation> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let (source, turn, _) = resolve(ledger, admitted, &intent.image)?;
        let profile = self
            .config
            .handwriting
            .profile
            .ok_or_else(|| anyhow::anyhow!("profile absent"))?;
        let correction = Uuid::new_v5(
            &profile,
            format!(
                "runtime-confirmed-v1:{}:{}:{}",
                turn.id, source.id, source.image.sha256
            )
            .as_bytes(),
        );
        let record = handwriting::record_id(profile, correction)
            .map_err(|_| anyhow::anyhow!("invalid correction identity"))?;
        let heads = ledger.store().heads(Namespace::Handwriting, record)?;
        ensure!(heads.len() <= 1, "conflicted correction");
        let expected = heads.iter().map(|e| e.revision_id).collect::<Vec<_>>();
        let confirmed_ms: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .try_into()?;
        let kind = if original.is_some() {
            handwriting::ProvenanceKind::CallerConfirmedSample
        } else {
            handwriting::ProvenanceKind::CallerConfirmedAfterAbstention
        };
        let enrollment = original.is_none();
        let first = expected.is_empty();
        let confirm = Box::new(handwriting::ConfirmInput {
            handwriting_version: 1,
            profile,
            correction,
            operation: admitted.operation,
            expected_heads: expected,
            confirmation: "user-confirmed".into(),
            original_text: original,
            corrected_text: intent.corrected_text.clone(),
            confirmed_ms,
            provenance: handwriting::Provenance {
                kind,
                description: "Explicit runtime handwriting confirmation; source IDs are caller assertions, not native revision authority.".into(),
                source: Some([admitted.conversation, turn.id, source.id]),
            },
            sample_png_base64: STANDARD.encode(png),
        });
        #[cfg(test)]
        let input = {
            let mut value = serde_json::to_value(&confirm)?;
            value["command"] = serde_json::json!(if first { "confirm" } else { "update" });
            serde_json::to_vec(&value)?
        };
        let request = if first {
            handwriting::Request::Confirm(confirm)
        } else {
            handwriting::Request::Update(confirm)
        };
        let validated = if enrollment {
            handwriting::validate_enrollment_request(request)
        } else {
            handwriting::validate_request(request)
        }
        .map_err(|_| anyhow::anyhow!("confirmed correction validation refused"))?;
        admitted.check(ledger, observe)?;
        resolve(ledger, admitted, &intent.image)?;
        // Reuse Dispatch's bounded event admission; unknown effects cannot be re-prepared.
        let event = writer::stage(admitted.operation, "handwriting-confirmation");
        ensure!(
            self.input_events.len() < 4096 && self.input_events.insert(event),
            "confirmation already prepared; use original status/input"
        );
        ensure!(
            ledger
                .store()
                .snapshot_revisions_matching(
                    &[
                        Namespace::Conversation,
                        Namespace::Source,
                        Namespace::Handwriting
                    ],
                    crate::storage::MAX_ITEMS,
                    |e| e.operation_id == admitted.operation
                )?
                .is_empty(),
            "confirmation operation already recorded; reconcile original status"
        );
        let owner = handwriting::LocalCorrections::new(ledger.store().clone(), self.config)
            .map_err(|_| anyhow::anyhow!("correction ownership unavailable"))?;
        Ok(PreparedConfirmation {
            #[cfg(test)]
            input,
            validated,
            owner,
            store: ledger.store().clone(),
            scope: RequestScope {
                conversation: admitted.conversation,
                operation: admitted.operation,
                root: admitted.root.clone(),
                source: admitted.source.clone(),
            },
            image: intent.image.clone(),
            normalized_source: intent.normalized_source,
        })
    }
}
