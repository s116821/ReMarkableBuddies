//! Deliberate stored-target generation through the existing regional handwriting path.
//! Historical references grant neither native input nor page mutation authority.
use super::*;
use crate::{
    config::Config,
    conversation::{
        target::{self, RecordRef, RegionSource, Understanding},
        ExpectedHeads,
    },
    retrieval::notebook,
    workflow::conversation_dispatch::Dispatch,
};
use std::collections::BTreeSet;

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    GenerateTarget {
        conversation: Uuid,
        operation: Uuid,
        expected_root: ExpectedHeads,
        target: RecordRef,
        request: String,
        guidance: Option<String>,
    },
}
impl Request {
    pub fn identities(&self) -> (Uuid, Uuid) {
        let Self::GenerateTarget {
            conversation,
            operation,
            ..
        } = self;
        (*conversation, *operation)
    }
    pub fn validate(&self) -> Result<()> {
        let Self::GenerateTarget {
            conversation,
            operation,
            expected_root,
            target,
            request,
            guidance,
        } = self;
        ensure!(
            !conversation.is_nil()
                && !operation.is_nil()
                && conversation != operation
                && expected_root.0.len() == 1
                && target.namespace == Namespace::Source
                && !target.record.is_nil()
                && !target.revision.is_nil(),
            "invalid target Writer identity"
        );
        validate_request(request)?;
        ensure!(
            guidance
                .as_ref()
                .is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0')),
            "invalid target Writer guidance"
        );
        Ok(())
    }
}
fn report(request: &Request, status: Status, failure: Option<Failure>) -> Report {
    let (conversation, operation) = request.identities();
    Report {
        status,
        conversation,
        operation,
        draft: None,
        failure,
        prepared_note: None,
    }
}

/// The input owner supplies one explicit operation and exact root/target, never
/// caller-supplied OCR or a replacement operation for an uncertain earlier call.
pub fn run<E: LLMEngine>(
    ledger: &Ledger,
    config: &Config,
    engine: &mut E,
    input: &Request,
    model_profile: &str,
    canceled: &mut dyn FnMut() -> bool,
) -> Report {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(300);
    if input.validate().is_err() || !config.admits_writer(false) {
        return report(input, Status::Refused, Some(Failure::InvalidInput));
    }
    let Some(owner) = Owner::acquire(ledger.store()) else {
        return report(input, Status::Busy, None);
    };
    let SelfParts {
        conversation,
        operation,
        root,
        reference,
        text,
        guidance,
    } = parts(input);
    let result = (|| -> Result<Report> {
        let dispatch = Dispatch::local(config, ledger)?;
        ensure!(
            ledger.expected(Namespace::Conversation, conversation)? == root,
            "target Writer root changed"
        );
        let (mut frame, target) = ledger.stored_target_frame(conversation, reference)?;
        ensure!(
            matches!(target.candidate.understanding, Understanding::Interpreted),
            "target is unresolved"
        );
        let geometry = target
            .candidate
            .geometry
            .as_ref()
            .context("bounded target absent")?;
        let stage = super::stage(operation, "target-normalization");
        ensure!(
            ledger
                .store()
                .value(Namespace::Conversation, stage)?
                .is_none()
                && ledger
                    .store()
                    .value(Namespace::Conversation, super::stage(operation, "user"))?
                    .is_none(),
            "target Writer already started; reconcile original operation"
        );
        let mut progress = || -> Result<()> {
            ensure!(
                !canceled() && Instant::now() < deadline - Duration::from_secs(60),
                "target Writer expired"
            );
            frame.check(ledger)
        };
        let masked =
            geometry.mask_png(frame.original(), frame.source().dimensions, &mut progress)?;
        let derived = crate::storage::Media {
            sha256: digest(&masked),
            bytes: masked.len() as u64,
            media_type: "image/png".into(),
        };
        let admitted = notebook::admit_target(1, masked.clone(), &mut |_| progress())?;
        let capture = dispatch.handwriting_runtime(ledger)?.map(|runtime| {
            runtime.capture(
                ledger.store(),
                &BTreeSet::from([
                    frame.source().original.sha256.clone(),
                    derived.sha256.clone(),
                ]),
                &mut || progress().map_err(|_| crate::handwriting::ContextFailure::Canceled),
            )
        });
        ensure!(
            capture.as_ref().is_none_or(|c| c.failure.is_none()),
            "target handwriting context unavailable"
        );
        progress()?;
        // A durable stage precedes the first POST. Reopen never repeats it.
        let prepared = ledger.append(
            super::stage(operation, "target-normalization-admission"),
            root,
            Turn {
                id: stage,
                conversation,
                exchange: operation,
                sequence: 0,
                role: Role::User,
                mode: Mode::Writer,
                outcome: Outcome::Prepared,
                text: None,
                sources: vec![],
                correction_of: None,
                created_ms: now(),
                updated_ms: now(),
                completion: None,
            },
            vec![],
        )?;
        frame.acknowledge(ledger, &prepared)?;
        let mut progress = || -> Result<()> {
            ensure!(
                !canceled() && Instant::now() < deadline - Duration::from_secs(60),
                "target Writer expired"
            );
            frame.check(ledger)
        };
        let document = ledger.target_document(&target)?;
        let (normalized, _) = notebook::normalize_admitted(
            document,
            &admitted,
            model_profile,
            capture.as_ref().and_then(|c| c.context.as_deref()),
            notebook::TranscriptionPolicy::WholePage,
            engine,
            &mut |_| progress(),
        );
        progress()?;
        let notebook::Normalized::Page(page) =
            normalized.map_err(|_| anyhow::anyhow!("target normalization refused"))?
        else {
            anyhow::bail!("target does not have complete regional text")
        };
        let mut document = page.document.clone();
        let extractor_revision = document.extractor_revision.clone();
        document.extractor_revision = format!(
            "region-v1:{}",
            digest(&serde_json::to_vec(&(reference, &extractor_revision))?)
        );
        let normalized = NormalizedTextSource::new(
            conversation,
            super::stage(operation, "user"),
            document.clone(),
            TextScope::Page { page: 1 },
            guidance.clone(),
        )?;
        let regional = RegionSource {
            id: normalized.id,
            conversation,
            turn: normalized.turn,
            target: reference.clone(),
            original: frame.source().original.clone(),
            derived: derived.clone(),
            dimensions: frame.source().dimensions,
            recipe: target::geometry::RECIPE.into(),
            normalization_profile: model_profile.into(),
            normalized_sha256: digest(&serde_json::to_vec(&document.units)?),
            about: target.candidate.about()?,
            normalized,
            normalization_proof: target::NormalizationProof {
                profile: serde_json::to_value(&page.profile)?,
                prompt_sha256: page.prompt_sha256.clone(),
                response_sha256: page.response_sha256.clone(),
                source_sha256: page.document.source_sha256.clone(),
                extractor_revision,
            },
        };
        regional.validate()?;
        ledger.stage_image(&derived, masked.as_slice())?;
        progress()?;
        let request = super::Request::Generate {
            conversation,
            operation,
            request: text.clone(),
            document,
            selection: TextScope::Page { page: 1 },
            guidance,
        };
        let preference = config.personalization_snapshot()?;
        let mut produced = None;
        // Writer's own preparation advances root and pins its exact regional source.
        // From this point its existing currentness guard owns root progression.
        let expected = frame.expected_root();
        let mut canceled = || canceled() || Instant::now() >= deadline - Duration::from_secs(60);
        Ok(super::run_owned_source(
            ledger,
            engine,
            &request,
            &preference,
            dispatch
                .local_memory()
                .as_deref()
                .map(|p| p as &dyn crate::memory::Provider),
            &mut canceled,
            OwnedRun {
                owner: &owner,
                expected_root: Some(expected),
                historical_images: false,
                produced: &mut produced,
            },
            Some(regional),
        ))
    })();
    result.unwrap_or_else(|_| report(input, Status::Refused, Some(Failure::Storage)))
}
struct SelfParts<'a> {
    conversation: Uuid,
    operation: Uuid,
    root: ExpectedHeads,
    reference: &'a RecordRef,
    text: &'a String,
    guidance: Option<String>,
}
fn parts(input: &Request) -> SelfParts<'_> {
    let Request::GenerateTarget {
        conversation,
        operation,
        expected_root,
        target,
        request,
        guidance,
    } = input;
    SelfParts {
        conversation: *conversation,
        operation: *operation,
        root: expected_root.clone(),
        reference: target,
        text: request,
        guidance: guidance.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Model {
        calls: Vec<Vec<String>>,
        parts: Vec<String>,
    }
    impl LLMEngine for Model {
        fn add_text_content(&mut self, s: &str) {
            self.parts.push(s.into());
        }
        fn add_image_content(&mut self, s: &str) {
            self.parts.push(format!("PNG:{s}"));
        }
        fn clear_content(&mut self) {
            self.parts.clear();
        }
        fn execute(&mut self) -> Result<String> {
            self.calls.push(self.parts.clone());
            Ok(if self.calls.len() == 1 {
                r#"{"pages":[{"page":1,"status":"transcribed","text":"Only selected equation x = 7"}]}"#.into()
            } else {
                "# Regional note\n\nOnly selected equation x = 7".into()
            })
        }
    }
    #[test]
    fn stored_target_writer_normalizes_once_reopens_refines_and_keeps_export_scope() {
        use crate::conversation::{target_source::tests::fixture, ContextBudget};
        let (_f, ledger, c, _) = fixture();
        let view = ledger
            .context(
                c,
                &ContextBudget {
                    max_text_bytes: 4096,
                    max_turns: 8,
                    provider_token_limit: Some(4096),
                },
                |_| Ok(1),
            )
            .unwrap();
        let reference = view.targets[0].reference.clone();
        let target_id = reference.record;
        let config = Config {
            paths: ledger.store().paths.clone(),
            ..Config::default()
        };
        let operation = Uuid::new_v4();
        let input = Request::GenerateTarget {
            conversation: c,
            operation,
            expected_root: ledger.expected(Namespace::Conversation, c).unwrap(),
            target: reference,
            request: "Make a regional note".into(),
            guidance: None,
        };
        let mut model = Model {
            calls: vec![],
            parts: vec![],
        };
        let report = run(
            &ledger,
            &config,
            &mut model,
            &input,
            "scripted-regional",
            &mut || false,
        );
        assert_eq!(
            report.status,
            Status::Generated,
            "{}",
            serde_json::to_string(&report).unwrap()
        );
        assert_eq!(model.calls.len(), 2);
        assert_eq!(
            model.calls[0]
                .iter()
                .filter(|s| s.starts_with("PNG:"))
                .count(),
            1
        );
        assert!(!model.calls[1].iter().any(|s| s.starts_with("PNG:")));
        assert!(model.calls[1]
            .iter()
            .any(|s| s.contains("all text recognized from exact masked region")));
        let draft = report.draft.unwrap();
        assert_eq!(
            run(
                &ledger,
                &config,
                &mut model,
                &input,
                "scripted-regional",
                &mut || false
            )
            .status,
            Status::Refused
        );
        assert_eq!(
            model.calls.len(),
            2,
            "repeated original must never normalize twice"
        );
        let paths = ledger.store().paths.clone();
        drop(ledger);
        let ledger = Ledger::new(std::sync::Arc::new(Store::open(paths).unwrap()));
        let refined = super::super::run(
            &ledger,
            &mut model,
            &super::super::Request::Refine {
                conversation: c,
                operation: Uuid::new_v4(),
                request: "Shorter".into(),
                draft,
            },
            &config.personalization_snapshot().unwrap(),
            &mut || false,
        );
        assert_eq!(
            refined.status,
            Status::Generated,
            "{}",
            serde_json::to_string(&refined).unwrap()
        );
        assert_eq!(model.calls.len(), 3);
        let prepared = super::super::run(
            &ledger,
            &mut model,
            &super::super::Request::PrepareExport {
                conversation: c,
                draft: refined.draft.unwrap(),
                correlation: Uuid::new_v4(),
                effect_operation: Uuid::new_v4(),
            },
            &config.personalization_snapshot().unwrap(),
            &mut || false,
        );
        assert_eq!(prepared.status, Status::PreparedExport);
        let note = prepared.prepared_note.unwrap();
        assert!(note.source_scope.ends_with(&format!(":target:{target_id}")));
        assert_eq!(note.markdown.matches("About: Original region").count(), 1);
        let records = ledger.inspect(c, false).unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r, Record::RegionSource(_)))
                .count(),
            1
        );
        assert!(
            !records
                .iter()
                .any(|r| matches!(r, Record::NormalizedText(_))),
            "region never masquerades as a legacy page source"
        );
        assert!(model.parts.is_empty());
    }
}
