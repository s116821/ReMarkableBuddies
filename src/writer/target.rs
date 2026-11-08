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
    pub(crate) fn with_root(&self, root: ExpectedHeads) -> Self {
        let mut request = self.clone();
        let Self::GenerateTarget { expected_root, .. } = &mut request;
        *expected_root = root;
        request
    }
    pub(crate) fn root(&self) -> &ExpectedHeads {
        let Self::GenerateTarget { expected_root, .. } = self;
        expected_root
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

/// One model slot of the original regional parent. OpenAI clamps its owned
/// request to this deadline; no slot can be reused after any attempted submit.
struct ModelStage<'a, E> {
    engine: &'a mut E,
    deadline: Instant,
    used: bool,
}
impl<E: LLMEngine> LLMEngine for ModelStage<'_, E> {
    fn guarded_timeout(&self) -> Option<Duration> {
        self.engine.guarded_timeout()
    }
    fn add_text_content(&mut self, s: &str) {
        self.engine.add_text_content(s)
    }
    fn add_image_content(&mut self, s: &str) {
        self.engine.add_image_content(s)
    }
    fn clear_content(&mut self) {
        self.engine.clear_content()
    }
    fn execute(&mut self) -> Result<String> {
        self.execute_with_progress(&mut || Ok(()))
    }
    fn execute_with_progress(
        &mut self,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<String> {
        ensure!(
            !std::mem::replace(&mut self.used, true) && Instant::now() < self.deadline,
            "regional model slot expired or consumed"
        );
        #[cfg(target_os = "linux")]
        {
            let deadline = self.deadline;
            let original = std::cell::RefCell::new(progress);
            self.engine.execute_with_deadline(
                &mut || (original.borrow_mut())(),
                &mut |submit| {
                    ensure!(Instant::now() < deadline, "regional model handoff expired");
                    (original.borrow_mut())()?;
                    submit()
                },
                deadline,
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = progress;
            anyhow::bail!("regional owned model adapter unavailable")
        }
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
    let deadline = Instant::now() + Duration::from_secs(300);
    let Some(owner) = Owner::acquire(ledger.store()) else {
        return report(input, Status::Busy, None);
    };
    let mut produced = None;
    run_owned(
        ledger,
        engine,
        input,
        canceled,
        TargetRun {
            config,
            model_profile,
            owner: &owner,
            deadline,
        },
        &mut produced,
    )
}
pub(crate) struct TargetRun<'a, 'store> {
    pub(crate) config: &'a Config,
    pub(crate) model_profile: &'a str,
    pub(crate) owner: &'a Owner<'store>,
    pub(crate) deadline: Instant,
}
pub(crate) fn run_owned<E: LLMEngine>(
    ledger: &Ledger,
    engine: &mut E,
    input: &Request,
    canceled: &mut dyn FnMut() -> bool,
    context: TargetRun<'_, '_>,
    produced: &mut Option<crate::conversation::WriterSelection>,
) -> Report {
    let TargetRun {
        config,
        model_profile,
        owner,
        deadline,
    } = context;
    if input.validate().is_err()
        || !config.admits_writer(false)
        || !std::ptr::eq(owner.0, ledger.store().as_ref())
    {
        return report(input, Status::Refused, Some(Failure::InvalidInput));
    }
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
        let (normalized, _) = {
            let mut recognition = ModelStage {
                engine,
                deadline: deadline - Duration::from_secs(150),
                used: false,
            };
            notebook::normalize_admitted(
                document,
                &admitted,
                model_profile,
                capture.as_ref().and_then(|c| c.context.as_deref()),
                notebook::TranscriptionPolicy::WholePage,
                &mut recognition,
                &mut |_| progress(),
            )
        };
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
        // Writer's own preparation advances root and pins its exact regional source.
        // From this point its existing currentness guard owns root progression.
        let expected = frame.expected_root();
        let mut canceled = || canceled() || Instant::now() >= deadline - Duration::from_secs(60);
        let mut generation = ModelStage {
            engine,
            deadline: deadline - Duration::from_secs(60),
            used: false,
        };
        Ok(super::run_owned_source(
            ledger,
            &mut generation,
            &request,
            super::ReceivingPreferences {
                personalization: &preference,
                memory: dispatch
                    .local_memory()
                    .as_deref()
                    .map(|p| p as &dyn crate::memory::Provider),
            },
            &mut canceled,
            OwnedRun {
                owner,
                expected_root: Some(expected),
                historical_images: false,
                produced,
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
        hook: Option<(usize, Box<dyn FnOnce()>)>,
        normalization: Option<String>,
    }
    impl LLMEngine for Model {
        fn guarded_timeout(&self) -> Option<Duration> {
            Some(Duration::from_secs(1))
        }
        fn execute_with_handoff(
            &mut self,
            progress: &mut dyn FnMut() -> Result<()>,
            handoff: &mut crate::llm::ModelHandoff<'_>,
        ) -> Result<String> {
            progress()?;
            let mut result = None;
            handoff(&mut || {
                result = Some(self.execute());
                Ok(())
            })?;
            result.context("fixture submit absent")?
        }
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
            if self
                .hook
                .as_ref()
                .is_some_and(|(at, _)| *at == self.calls.len())
            {
                let (_, hook) = self.hook.take().unwrap();
                hook();
            }
            if self.calls.len() == 1 {
                if let Some(response) = self.normalization.take() {
                    return Ok(response);
                }
            }
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
            hook: None,
            normalization: None,
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
    #[test]
    fn changed_config_or_capabilities_during_normalization_stop_before_generation() {
        for capabilities in [false, true] {
            let (f, ledger, c, _) = crate::conversation::target_source::tests::fixture();
            let config = Config {
                paths: ledger.store().paths.clone(),
                ..Default::default()
            };
            let path = f.0.join("config.json");
            std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
            drop(ledger);
            let config = Config::load(Some(path.clone())).unwrap();
            let ledger = Ledger::new(std::sync::Arc::new(
                Store::open_source_target_startup(&config).unwrap(),
            ));
            let context = ledger
                .context(
                    c,
                    &crate::conversation::ContextBudget {
                        max_text_bytes: 4096,
                        max_turns: 8,
                        provider_token_limit: Some(4096),
                    },
                    |_| Ok(1),
                )
                .unwrap();
            let request = Request::GenerateTarget {
                conversation: c,
                operation: Uuid::new_v4(),
                expected_root: ledger.expected(Namespace::Conversation, c).unwrap(),
                target: context.targets[0].reference.clone(),
                request: "Make a note".into(),
                guidance: None,
            };
            let changed = if capabilities {
                config.paths.data.join("capabilities.json")
            } else {
                path
            };
            let mut model = Model {
                calls: vec![],
                parts: vec![],
                normalization: None,
                hook: Some((
                    1,
                    Box::new(move || {
                        let mut bytes = std::fs::read(&changed).unwrap();
                        bytes.push(b' ');
                        std::fs::write(changed, bytes).unwrap();
                    }),
                )),
            };
            let report = run(
                &ledger,
                &config,
                &mut model,
                &request,
                "fixture",
                &mut || false,
            );
            assert_ne!(report.status, Status::Generated);
            assert_eq!(
                model.calls.len(),
                1,
                "changed original policy must not pay for the next model stage"
            );
            assert!(model.parts.is_empty());
            assert!(!ledger.inspect(c,false).unwrap().iter().any(|r|matches!(r,Record::Turn(t) if t.mode==Mode::Writer && t.outcome==Outcome::Generated)));
        }
    }
    #[test]
    fn regional_target_reuses_one_confirmed_context_and_reopens_its_exact_profile_identity() {
        use crate::{
            conversation::{target_source::tests::fixture, ContextBudget},
            handwriting::{self, Provider},
        };
        use base64::Engine as _;
        let (_f, ledger, c, _) = fixture();
        let profile = Uuid::new_v4();
        let config = Config {
            paths: ledger.store().paths.clone(),
            handwriting: handwriting::Configuration {
                enabled: true,
                profile: Some(profile),
            },
            ..Default::default()
        };
        let mut sample = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4)
            .write_to(&mut sample, image::ImageFormat::Png)
            .unwrap();
        let sample = base64::engine::general_purpose::STANDARD.encode(sample.into_inner());
        let input = serde_json::json!({"handwriting_version":1,"command":"confirm","profile":profile,"correction":Uuid::new_v4(),"operation":Uuid::new_v4(),"expected_heads":[],"confirmation":"user-confirmed","original_text":"SYNTHETIC ORIGINAL","corrected_text":"SYNTHETIC CONFIRMED","confirmed_ms":1,"provenance":{"kind":"caller-confirmed-sample","description":"Synthetic regional Writer context fixture","source":null},"sample_png_base64":sample});
        let validated = handwriting::parse(&mut std::io::Cursor::new(
            serde_json::to_vec(&input).unwrap(),
        ))
        .unwrap();
        drop(
            handwriting::LocalCorrections::new(ledger.store().clone(), &config)
                .unwrap()
                .execute(validated),
        );
        let context = ledger
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
        let request = Request::GenerateTarget {
            conversation: c,
            operation: Uuid::new_v4(),
            expected_root: ledger.expected(Namespace::Conversation, c).unwrap(),
            target: context.targets[0].reference.clone(),
            request: "Make a regional note".into(),
            guidance: None,
        };
        let mut model = Model {
            calls: vec![],
            parts: vec![],
            hook: None,
            normalization: None,
        };
        let generated = run(
            &ledger,
            &config,
            &mut model,
            &request,
            "fixture-confirmed",
            &mut || false,
        );
        assert_eq!(
            generated.status,
            Status::Generated,
            "{}",
            serde_json::to_string(&generated).unwrap()
        );
        assert_eq!(model.calls.len(), 2);
        assert_eq!(
            model.calls[0]
                .iter()
                .filter(|p| p.starts_with("PNG:"))
                .count(),
            2
        );
        assert!(model.calls[0]
            .iter()
            .any(|p| p.contains("SYNTHETIC CONFIRMED")));
        assert!(!model.calls[1]
            .iter()
            .any(|p| p.starts_with("PNG:") || p.contains("SYNTHETIC CONFIRMED")));
        let region = ledger
            .inspect(c, false)
            .unwrap()
            .into_iter()
            .find_map(|r| {
                if let Record::RegionSource(r) = r {
                    Some(r)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(region.normalization_proof.profile["protocol"], 2);
        assert_eq!(
            region.normalization_proof.profile["confirmed_context"]["profile"],
            profile.to_string()
        );
        let paths = ledger.store().paths.clone();
        drop(ledger);
        let ledger = Ledger::new(std::sync::Arc::new(Store::open(paths).unwrap()));
        ledger
            .regional_source_proof(&region, &mut || Ok(()))
            .unwrap();
    }
    #[test]
    fn unreadable_and_partial_target_text_never_pay_for_generation() {
        for response in [
            r#"{"pages":[{"page":1,"status":"unreadable","text":""}]}"#,
            r#"{"pages":[{"page":1,"status":"partial","spans":[{"kind":"known","text":"x"},{"kind":"uncertain"}]}]}"#,
        ] {
            let (_f, ledger, c, _) = crate::conversation::target_source::tests::fixture();
            let config = Config {
                paths: ledger.store().paths.clone(),
                ..Default::default()
            };
            let context = ledger
                .context(
                    c,
                    &crate::conversation::ContextBudget {
                        max_text_bytes: 4096,
                        max_turns: 8,
                        provider_token_limit: Some(4096),
                    },
                    |_| Ok(1),
                )
                .unwrap();
            let input = Request::GenerateTarget {
                conversation: c,
                operation: Uuid::new_v4(),
                expected_root: ledger.expected(Namespace::Conversation, c).unwrap(),
                target: context.targets[0].reference.clone(),
                request: "Make regional notes".into(),
                guidance: None,
            };
            let mut model = Model {
                calls: vec![],
                parts: vec![],
                hook: None,
                normalization: Some(response.into()),
            };
            let report = run(&ledger, &config, &mut model, &input, "fixture", &mut || {
                false
            });
            assert_ne!(report.status, Status::Generated);
            assert_eq!(model.calls.len(), 1);
            assert!(model.parts.is_empty());
            assert!(
                !ledger
                    .inspect(c, false)
                    .unwrap()
                    .iter()
                    .any(|r| matches!(r, Record::RegionSource(_))),
                "incomplete text never becomes a complete regional source"
            );
        }
    }
}

#[cfg(test)]
mod proof_tests {
    use super::*;
    use crate::conversation::{target_source::tests::fixture, ContextBudget};
    use std::io::Cursor;
    #[test]
    fn saved_valid_png_with_wrong_mask_refuses_even_with_consistent_normalized_input_hash() {
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
        let target = view.targets[0].reference.clone();
        let original = &view.targets[0].target.source;
        let document = ledger.target_document(&view.targets[0].target).unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([255, 255, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let png = png.into_inner();
        let derived = crate::storage::Media {
            sha256: digest(&png),
            bytes: png.len() as u64,
            media_type: "image/png".into(),
        };
        // An unrelated committed image is valid data, but never proof of a region.
        let envelope = crate::storage::Envelope {
            envelope_version: 1,
            namespace: Namespace::Source,
            domain_schema_version: 1,
            record_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            parents: Default::default(),
            operation_id: Uuid::new_v4(),
            actor_id: ledger.store().actor_id,
            kind: crate::storage::Kind::Value,
            payload: serde_json::json!({"fixture":"unrelated historical image"}),
            media_descriptors: vec![derived.clone()],
        };
        ledger
            .store()
            .commit(
                vec![envelope],
                std::collections::BTreeMap::from([(derived.sha256.clone(), png.clone())]),
            )
            .unwrap();
        let admitted = notebook::admit_target(1, png, &mut |_| Ok(())).unwrap();
        let profile = serde_json::json!({"protocol":1,"prompt_revision":"transcribe-visible-v2","caller_model_profile":"fixture"});
        let extractor_revision = format!(
            "notebook-ocr-unverified-v2:{}",
            digest(&notebook::profile_bytes(&profile).unwrap())
        );
        let normalized = NormalizedTextSource::new(
            c,
            Uuid::new_v4(),
            NormalizedDocument {
                document,
                format: crate::retrieval::SourceFormat::Notebook,
                source_sha256: admitted.source_sha256().into(),
                extractor_revision: format!(
                    "region-v1:{}",
                    digest(&serde_json::to_vec(&(&target, &extractor_revision)).unwrap())
                ),
                units: vec![crate::retrieval::SourceUnit {
                    page: Some(1),
                    section: None,
                    text: "Unverified regional text".into(),
                }],
            },
            TextScope::Page { page: 1 },
            None,
        )
        .unwrap();
        let region = RegionSource {
            id: normalized.id,
            conversation: c,
            turn: normalized.turn,
            target,
            original: original.original.clone(),
            derived,
            dimensions: [2, 2],
            recipe: target::geometry::RECIPE.into(),
            normalization_profile: "fixture".into(),
            normalized_sha256: digest(&serde_json::to_vec(&normalized.document.units).unwrap()),
            about: "About: Original region".into(),
            normalization_proof: target::NormalizationProof {
                profile,
                prompt_sha256: digest(b"fixture prompt"),
                response_sha256: digest(b"fixture response"),
                source_sha256: normalized.document.source_sha256.clone(),
                extractor_revision,
            },
            normalized,
        };
        region.validate().unwrap();
        let error = ledger
            .regional_source_proof(&region, &mut || Ok(()))
            .unwrap_err();
        assert!(error.to_string().contains("mask/recipe"), "{error}");
    }
}
