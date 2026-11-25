//! One private retained-frame resolver. Serialized references alone admit no use.
use super::{target::*, *};
use crate::storage::{digest, ObjectRef};
use std::sync::{Arc, Weak};
pub(crate) struct SourceFrame {
    store: Weak<Store>,
    generation: Uuid,
    mode_pin: crate::storage::sync::mode::SyncModeToken,
    ready_pin: Option<String>,
    checks: std::sync::Mutex<crate::storage::selection::SelectedReadBudget>,
    state_heads: ExpectedHeads,
    pub(super) root: ExpectedHeads,
    pub(super) source: RetainedSourceRef,
    pub(super) original: Vec<u8>,
    pub(super) conversation: Uuid,
}
pub(crate) fn descriptor(
    reference: RecordRef,
    record: &Record,
) -> Result<(RetainedSourceRef, Uuid)> {
    let (kind, conversation, turn, original, dimensions, overview, overview_dimensions) =
        match record {
            Record::Source(v) => {
                let (original, dimensions) = v
                    .parent
                    .as_ref()
                    .map(|m| (m.clone(), v.parent_dimensions))
                    .unwrap_or((v.image.clone(), Some([v.width, v.height])));
                (
                    CaptureKind::ImageUse,
                    v.conversation,
                    v.turn,
                    original,
                    dimensions.context("target source parent dimensions absent")?,
                    v.image.clone(),
                    [v.width, v.height],
                )
            }
            Record::ResidentCapture(v) => {
                let overview = v
                    .images
                    .first()
                    .context("target resident overview absent")?;
                (
                    CaptureKind::Resident,
                    v.conversation,
                    v.turn,
                    v.original_png.clone(),
                    v.original_dimensions,
                    overview.media.clone(),
                    overview.dimensions,
                )
            }
            Record::DevelopmentCapture(v) => {
                let overview = v
                    .images
                    .first()
                    .context("target development overview absent")?;
                (
                    CaptureKind::Development,
                    v.conversation,
                    v.turn,
                    v.original_png.clone(),
                    v.original_dimensions,
                    overview.media.clone(),
                    overview.dimensions,
                )
            }
            Record::LegacyCapture(v) => {
                let overview = v
                    .images
                    .iter()
                    .find(|i| i.role == LegacyImageRole::Overview)
                    .context("target legacy overview absent")?;
                let original = v.parent.as_ref().unwrap_or(overview);
                (
                    CaptureKind::Legacy,
                    v.conversation,
                    v.turn,
                    original.media.clone(),
                    original.dimensions,
                    overview.media.clone(),
                    overview.dimensions,
                )
            }
            Record::Capture(v) => {
                let overview = v
                    .facts
                    .images
                    .iter()
                    .find(|i| i.role == capture_facts::Role::Overview)
                    .context("target SDK overview absent")?;
                (
                    CaptureKind::Sdk,
                    v.conversation,
                    v.turn,
                    v.facts.native_parent.media(),
                    v.facts.native_parent.dimensions,
                    overview.media(),
                    overview.dimensions,
                )
            }
            _ => bail!("target source is not a retained full-frame capture"),
        };
    let value = RetainedSourceRef {
        kind,
        record: reference,
        turn,
        original,
        dimensions,
        overview,
        overview_dimensions,
    };
    value.validate_record(record, conversation)?;
    Ok((value, conversation))
}
impl SourceFrame {
    pub(crate) fn check_current(&self) -> Result<()> {
        let store = self.store.upgrade().context("target frame Store expired")?;
        ensure!(
            store.source_target_generation()? == Some(self.generation),
            "target generation changed"
        );
        let mut reads = self
            .checks
            .lock()
            .map_err(|_| anyhow::anyhow!("target original check allowance unavailable"))?;
        store.check_runtime_policy(&mut reads)?;
        ensure!(
            store.target_runtime_pin(&mut reads)? == self.ready_pin
                && store.sync_mode_snapshot()?.token == self.mode_pin,
            "target ready policy or mode changed"
        );
        drop(reads);
        let heads = store
            .heads(Namespace::Conversation, self.conversation)?
            .iter()
            .map(|e| e.revision_id)
            .collect::<BTreeSet<_>>();
        ensure!(heads == self.root.0, "target root changed");
        let sources = store.heads(Namespace::Source, self.source.record.record)?;
        ensure!(
            sources.len() == 1 && sources[0].revision_id == self.source.record.revision,
            "target original source changed"
        );
        let state = store
            .heads(Namespace::Conversation, TargetState::id(self.conversation))?
            .iter()
            .map(|e| e.revision_id)
            .collect::<BTreeSet<_>>();
        ensure!(state == self.state_heads.0, "target state changed");
        Ok(())
    }
    pub(crate) fn check(&self, ledger: &Ledger) -> Result<()> {
        let store = self.store.upgrade().context("target frame Store expired")?;
        ensure!(
            Arc::ptr_eq(&store, ledger.store())
                && ledger.expected(Namespace::Conversation, self.conversation)? == self.root,
            "target root changed"
        );
        self.check_current()
    }
    pub(crate) fn acknowledge(&mut self, ledger: &Ledger, result: &WriteResult) -> Result<()> {
        self.root = ExpectedHeads(BTreeSet::from([result.historical.applied_root_revision]));
        self.state_heads =
            ledger.expected(Namespace::Conversation, TargetState::id(self.conversation))?;
        self.check(ledger)
    }
    #[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
    pub(crate) fn advance_captured_root(
        &mut self,
        ledger: &Ledger,
        previous: &ExpectedHeads,
        next: ExpectedHeads,
    ) -> Result<()> {
        ensure!(&self.root == previous, "target capture root changed");
        self.root = next;
        self.check(ledger)
    }

    pub(crate) fn expected_root(&self) -> ExpectedHeads {
        self.root.clone()
    }
    pub(crate) fn original(&self) -> &[u8] {
        &self.original
    }
    pub(crate) fn source(&self) -> &RetainedSourceRef {
        &self.source
    }
}
impl Ledger {
    pub(crate) fn target_source_frame(
        &self,
        conversation: Uuid,
        capture: Uuid,
    ) -> Result<SourceFrame> {
        ensure!(
            self.store.ordinary_reader_family()?,
            "selected target frame requires selected admission"
        );
        let generation = self
            .store
            .source_target_generation()?
            .context("source targets not prepared")?;
        let root = self.expected(Namespace::Conversation, conversation)?;
        ensure!(root.0.len() == 1, "target conversation root absent");
        let envelope = self
            .store
            .value(Namespace::Source, capture)?
            .context("target original source absent")?;
        let reference = RecordRef {
            namespace: envelope.namespace,
            record: envelope.record_id,
            revision: envelope.revision_id,
            object: self.store.notebook_revision_reference(&envelope)?,
        };
        let record = Self::decode(&envelope)?;
        let (source, owner) = descriptor(reference, &record)?;
        ensure!(owner == conversation, "foreign target capture");
        let user = self
            .store
            .value(Namespace::Conversation, source.turn)?
            .context("target source user absent")?;
        let Record::Turn(user) = Self::decode(&user)? else {
            bail!("target source user variant")
        };
        ensure!(
            user.conversation == conversation
                && user.role == Role::User
                && user.sources.contains(&capture),
            "target source/user association differs"
        );
        let original = match &record {
            Record::ResidentCapture(_) => self.stored_resident_images(capture)?.original_png,
            Record::DevelopmentCapture(_) => self.stored_development_images(capture)?.original_png,
            Record::Capture(_) => self.stored_sdk_images(capture)?.native_parent.bytes,
            Record::LegacyCapture(_) => {
                let captured = self.stored_legacy_images(capture)?;
                captured
                    .parent
                    .unwrap_or_else(|| captured.images[0].clone())
            }
            Record::Source(_) => self.store.read_object(&ObjectRef {
                sha256: source.original.sha256.clone(),
                bytes: source.original.bytes,
            })?,
            _ => unreachable!(),
        };
        Self::validate_image_bytes(
            &source.original,
            &original,
            source.dimensions[0],
            source.dimensions[1],
        )?;
        ensure!(
            digest(&original) == source.original.sha256,
            "target original bytes differ"
        );
        let mut reads = crate::storage::selection::SelectedReadBudget::new(
            0,
            32768,
            crate::storage::MAX_RECORD as u64 * 8192,
            0,
        );
        self.store.check_runtime_policy(&mut reads)?;
        let ready_pin = self.store.target_runtime_pin(&mut reads)?;
        let frame = SourceFrame {
            mode_pin: self.store.sync_mode_snapshot()?.token,
            ready_pin,
            checks: std::sync::Mutex::new(reads),
            store: Arc::downgrade(self.store()),
            generation,
            state_heads: self.expected(Namespace::Conversation, TargetState::id(conversation))?,
            root,
            source,
            original,
            conversation,
        };
        frame.check(self)?;
        Ok(frame)
    }
}

impl Ledger {
    pub(crate) fn target_interpretation_records(
        &self,
        user: &Turn,
        frame: &SourceFrame,
        candidate: &Candidate,
        installed_capture: Option<&Envelope>,
    ) -> Result<Vec<Envelope>> {
        frame.check(self)?;
        candidate.validate()?;
        ensure!(
            digest(&frame.original) == frame.source.original.sha256
                && frame.original.len() as u64 == frame.source.original.bytes,
            "owned original changed before interpretation"
        );
        ensure!(
            user.role == Role::User
                && user.outcome == Outcome::Interpreted
                && user.conversation == frame.conversation
                && user.sources.contains(&frame.source.record.record),
            "target interpretation user differs"
        );
        let reference = |envelope: &Envelope| -> Result<RecordRef> {
            let bytes = serde_json::to_vec(envelope)?;
            Ok(RecordRef {
                namespace: envelope.namespace,
                record: envelope.record_id,
                revision: envelope.revision_id,
                object: ObjectRef {
                    sha256: digest(&bytes),
                    bytes: bytes.len() as u64,
                },
            })
        };
        let mut source = frame.source.clone();
        if let Some(capture) = installed_capture {
            ensure!(
                capture.namespace == Namespace::Source
                    && capture.record_id == source.record.record
                    && capture.parents == BTreeSet::from([source.record.revision]),
                "installed target changed its original capture"
            );
            source.record = reference(capture)?;
        }
        let id = Uuid::new_v5(&user.id, b"urn:rmb:source-target:v1");
        let value = SourceTarget {
            id,
            conversation: user.conversation,
            introduced_by: user.id,
            source,
            candidate: candidate.clone(),
            previous: None,
        };
        let target = self.envelope(
            Namespace::Source,
            id,
            ExpectedHeads::default(),
            Record::SourceTarget(Box::new(value.clone())),
            value.media(),
        )?;
        let state_id = TargetState::id(user.conversation);
        let expected_state = self.expected(Namespace::Conversation, state_id)?;
        ensure!(expected_state.0.len() <= 1, "target state conflicted");
        let used = TargetUse {
            id: TargetUse::id(user.id),
            conversation: user.conversation,
            turn: user.id,
            action: if self
                .store
                .value(Namespace::Conversation, state_id)?
                .is_none_or(|state| state.parents.is_empty())
            {
                Action::Introduce
            } else {
                Action::Retarget
            },
            target: Some(reference(&target)?),
            about: Some(candidate.about()?),
            inline_body_sha256: None,
        };
        let used = self.envelope(
            Namespace::Conversation,
            used.id,
            self.expected(Namespace::Conversation, used.id)?,
            Record::TargetUse(Box::new(used)),
            vec![],
        )?;
        let state = TargetState {
            id: state_id,
            conversation: user.conversation,
            use_reference: reference(&used)?,
        };
        let state = self.envelope(
            Namespace::Conversation,
            state_id,
            expected_state,
            Record::TargetState(Box::new(state)),
            vec![],
        )?;
        Ok(vec![target, used, state])
    }
    pub(crate) fn advance_with_source_target(
        &self,
        operation: Uuid,
        expected_turn: ExpectedHeads,
        turn: Turn,
        frame: &mut SourceFrame,
        candidate: &Candidate,
        installed_center: Option<crate::analysis::SelectionCenter>,
    ) -> Result<WriteResult> {
        self.advance_with_captured_source_target(
            operation,
            expected_turn,
            turn,
            frame,
            candidate,
            installed_center,
            None,
            None,
        )
        .map(|(result, _)| result)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn advance_with_captured_source_target(
        &self,
        operation: Uuid,
        expected_turn: ExpectedHeads,
        turn: Turn,
        frame: &mut SourceFrame,
        candidate: &Candidate,
        installed_center: Option<crate::analysis::SelectionCenter>,
        captured: Option<&crate::storage::captured::Captured>,
        marked: Option<super::marked_question::PreparedMark>,
    ) -> Result<(WriteResult, Vec<crate::storage::Envelope>)> {
        let capture_id = frame.source.record.record;
        let selection = installed_center.map(|center| {
            let subject = if matches!(candidate.understanding, Understanding::Interpreted)
                && turn
                    .text
                    .as_ref()
                    .is_some_and(|question| !candidate.subject.eq_ignore_ascii_case(question))
            {
                crate::analysis::ReaderSourceSubject::try_from(candidate.subject.clone())
                    .ok()
                    .map(crate::analysis::ReaderSubject::Recognized)
            } else {
                None
            };
            crate::analysis::ReaderSelection { center, subject }
        });
        let resident_target = selection.as_ref().map(|selection| (capture_id, selection));
        let mut published = Vec::new();
        let (result, _) = self.advance_with_capture(
            operation,
            frame.expected_root(),
            expected_turn,
            turn,
            None,
            Some((frame, candidate)),
            resident_target,
            captured,
            marked,
            (resident_target.is_some() || captured.is_some()).then_some(&mut published),
        )?;
        if resident_target.is_some() {
            // The live frame and the saved source target must name the same
            // capture revision, including the center written in this transaction.
            let capture = published
                .iter()
                .find(|record| {
                    record.namespace == Namespace::Source && record.record_id == capture_id
                })
                .context("installed target did not publish a fresh capture")?;
            frame.source.record.object = self.store.notebook_revision_reference(capture)?;
            frame.source.record.revision = capture.revision_id;
        }
        frame.acknowledge(self, &result)?;
        Ok((result, published))
    }
}
impl Ledger {
    pub(crate) fn target_pending_records(&self, user: &Turn) -> Result<Vec<Envelope>> {
        let use_id = TargetUse::id(user.id);
        ensure!(
            self.expected(Namespace::Conversation, use_id)?.0.is_empty(),
            "new source already has target use"
        );
        let used = TargetUse {
            id: use_id,
            conversation: user.conversation,
            turn: user.id,
            action: Action::None,
            target: None,
            about: None,
            inline_body_sha256: None,
        };
        let used = self.envelope(
            Namespace::Conversation,
            use_id,
            ExpectedHeads::default(),
            Record::TargetUse(Box::new(used)),
            vec![],
        )?;
        let bytes = serde_json::to_vec(&used)?;
        let state = TargetState {
            id: TargetState::id(user.conversation),
            conversation: user.conversation,
            use_reference: RecordRef {
                namespace: Namespace::Conversation,
                record: use_id,
                revision: used.revision_id,
                object: ObjectRef {
                    sha256: digest(&bytes),
                    bytes: bytes.len() as u64,
                },
            },
        };
        let state = self.envelope(
            Namespace::Conversation,
            state.id,
            self.expected(Namespace::Conversation, state.id)?,
            Record::TargetState(Box::new(state)),
            vec![],
        )?;
        Ok(vec![used, state])
    }
    pub(crate) fn target_presentation_records(
        &self,
        user: Uuid,
        body: &str,
    ) -> Result<Vec<Envelope>> {
        let id = TargetUse::id(user);
        let original = self
            .store
            .value(Namespace::Conversation, id)?
            .context("target answer use absent")?;
        let Record::TargetUse(mut used) = Self::decode(&original)? else {
            bail!("target answer use variant")
        };
        let about = used.about.as_ref().context("target answer About absent")?;
        ensure!(
            body.starts_with(&format!("{about}\n")),
            "target answer body lost original About"
        );
        let body_hash = digest(body.as_bytes());
        ensure!(
            used.inline_body_sha256
                .as_ref()
                .is_none_or(|hash| *hash == body_hash),
            "target About body changed"
        );
        used.inline_body_sha256 = Some(body_hash);
        Ok(vec![self.envelope(
            Namespace::Conversation,
            id,
            ExpectedHeads(BTreeSet::from([original.revision_id])),
            Record::TargetUse(used),
            vec![],
        )?])
    }
    pub(crate) fn append_target_answer(
        &self,
        operation: Uuid,
        expected: ExpectedHeads,
        turn: Turn,
        user: Uuid,
    ) -> Result<WriteResult> {
        let fingerprint = Self::fingerprint(&("append-target-answer-v1", &turn, user))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        self.append_unreplayed_target(operation, expected, turn, vec![], fingerprint, Some(user))
    }
}
impl Ledger {
    pub(super) fn current_target_reference(&self, conversation: Uuid) -> Result<Option<RecordRef>> {
        let Some(state) = self
            .store
            .value(Namespace::Conversation, TargetState::id(conversation))?
        else {
            return Ok(None);
        };
        let Record::TargetState(state) = Self::decode(&state)? else {
            bail!("target state unavailable")
        };
        let original = &state.use_reference;
        let bytes = self.store.read_object(&original.object)?;
        let envelope: Envelope = serde_json::from_slice(&bytes)?;
        ensure!(
            envelope.namespace == original.namespace
                && envelope.record_id == original.record
                && envelope.revision_id == original.revision,
            "target state's exact use differs"
        );
        let Record::TargetUse(prior) = Self::decode(&envelope)? else {
            bail!("target state's use variant")
        };
        ensure!(
            state.conversation == conversation && prior.conversation == conversation,
            "foreign current target"
        );
        Ok(prior.target)
    }

    pub(crate) fn current_resident_writer_target(
        &self,
        conversation: Uuid,
        document: Uuid,
        page: Uuid,
    ) -> Result<Option<RecordRef>> {
        let Some(reference) = self.current_target_reference(conversation)? else {
            return Ok(None);
        };
        let target = self.stored_source_target(conversation, &reference)?;
        if !matches!(target.candidate.understanding, Understanding::Interpreted) {
            return Ok(None);
        }
        let source = &target.source.record;
        let bytes = self.store.read_object(&source.object)?;
        let envelope: Envelope = serde_json::from_slice(&bytes)?;
        ensure!(
            envelope.namespace == source.namespace
                && envelope.record_id == source.record
                && envelope.revision_id == source.revision,
            "current target capture reference differs"
        );
        let record = Self::decode(&envelope)?;
        target.source.validate_record(&record, conversation)?;
        let Record::ResidentCapture(capture) = record else {
            return Ok(None);
        };
        Ok(
            (capture.before.document == document && capture.before.page == page)
                .then_some(reference),
        )
    }

    pub(crate) fn target_inherited_records(&self, user: &Turn) -> Result<Vec<Envelope>> {
        let Some(target) = self.current_target_reference(user.conversation)? else {
            return Ok(vec![]);
        };
        let used = TargetUse {
            id: TargetUse::id(user.id),
            conversation: user.conversation,
            turn: user.id,
            action: Action::Inherit,
            target: Some(target),
            about: None,
            inline_body_sha256: None,
        };
        Ok(vec![self.envelope(
            Namespace::Conversation,
            used.id,
            ExpectedHeads::default(),
            Record::TargetUse(Box::new(used)),
            vec![],
        )?])
    }
}
impl Ledger {
    pub(crate) fn target_views(&self, uses: &[TargetUse]) -> Result<Vec<TargetView>> {
        let mut views = Vec::new();
        let mut seen = BTreeSet::new();
        let mut remaining = 8 * 1024 * 1024u64;
        for used in uses {
            let Some(reference) = &used.target else {
                continue;
            };
            if !seen.insert(reference.revision) {
                continue;
            }
            let mut exact = |reference: &RecordRef| -> Result<Record> {
                remaining = remaining
                    .checked_sub(reference.object.bytes)
                    .context("target history metadata budget exceeded")?;
                let encoded = self.store.read_object(&reference.object)?;
                let envelope: Envelope = serde_json::from_slice(&encoded)?;
                ensure!(
                    envelope.namespace == reference.namespace
                        && envelope.record_id == reference.record
                        && envelope.revision_id == reference.revision
                        && self.store.notebook_revision_reference(&envelope)? == reference.object,
                    "target exact committed reference differs"
                );
                Self::decode(&envelope)
            };
            let Record::SourceTarget(target) = exact(reference)? else {
                bail!("target view exact revision absent")
            };
            let source = exact(&target.source.record)?;
            views.push(TargetView::new(reference.clone(), *target, source)?);
        }
        Ok(views)
    }
    pub(crate) fn complete_target_view(&self, view: &mut ContextView) -> Result<()> {
        view.targets = self.target_views(&view.target_uses)?;
        for target in &view.targets {
            for media in target.media()? {
                if self
                    .store
                    .open_object(&ObjectRef {
                        sha256: media.sha256.clone(),
                        bytes: media.bytes,
                    })
                    .is_err()
                    && !view.missing_media.contains(&media)
                {
                    view.missing_media.push(media);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::Config;
    use std::io::Cursor;
    pub(crate) struct Fixture(pub std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub(crate) fn fixture() -> (Fixture, Ledger, Uuid, Uuid) {
        let f = Fixture(std::env::temp_dir().join(format!("rmb-target-{}", Uuid::new_v4())));
        let config = Config {
            paths: crate::storage::StorePaths {
                data: f.0.join("data"),
                cache: f.0.join("cache"),
                credentials: f.0.join("secrets"),
            },
            ..Default::default()
        };
        let ledger = Ledger::new(Arc::new(
            Store::open_source_target_startup(&config).unwrap(),
        ));
        let (c, source) = populate(&ledger);
        (f, ledger, c, source)
    }
    pub(crate) fn populate(ledger: &Ledger) -> (Uuid, Uuid) {
        let c = Uuid::new_v4();
        ledger.create(c, Uuid::new_v4(), 1).unwrap();
        let mut user = user(c);
        user.outcome = Outcome::Prepared;
        user.text = None;
        let source = Uuid::new_v4();
        user.sources.push(source);
        let png = {
            let mut b = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(2, 2)
                .write_to(&mut b, image::ImageFormat::Png)
                .unwrap();
            b.into_inner()
        };
        let media = Media {
            sha256: digest(&png),
            bytes: png.len() as u64,
            media_type: "image/png".into(),
        };
        ledger.stage_image(&media, png.as_slice()).unwrap();
        let image = ImageUse {
            id: source,
            conversation: c,
            turn: user.id,
            image: media,
            observation: SourceObservation {
                document: Uuid::new_v4(),
                page: Uuid::new_v4(),
                session: "synthetic".into(),
                visit: "synthetic".into(),
                content_revision: None,
                order_revision: None,
                capability_revision: "synthetic".into(),
                evidence_procedure: "unit-original".into(),
            },
            width: 2,
            height: 2,
            purpose: "overview".into(),
            ordinal: 0,
            parent: None,
            parent_dimensions: None,
            crop: None,
            transform: [0.5, 0., 0., 0.5, 0., 0.],
            target: None,
            captured_ms: 1,
        };
        ledger
            .append(
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, c).unwrap(),
                user.clone(),
                vec![image],
            )
            .unwrap();
        let mut frame = ledger.target_source_frame(c, source).unwrap();
        user.outcome = Outcome::Interpreted;
        user.text = Some("Explain this".into());
        ledger
            .advance_with_source_target(
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, user.id).unwrap(),
                user,
                &mut frame,
                &Candidate {
                    subject: "Original region".into(),
                    geometry: Some(geometry::Geometry {
                        components: vec![vec![
                            [0, 0],
                            [1_000_000, 0],
                            [1_000_000, 1_000_000],
                            [0, 1_000_000],
                        ]],
                    }),
                    understanding: Understanding::Interpreted,
                },
                None,
            )
            .unwrap();
        (c, source)
    }
    pub(crate) fn user(c: Uuid) -> Turn {
        Turn {
            id: Uuid::new_v4(),
            conversation: c,
            exchange: Uuid::new_v4(),
            sequence: 0,
            role: Role::User,
            mode: Mode::Reader,
            outcome: Outcome::Interpreted,
            text: Some("Explain this".into()),
            sources: vec![],
            correction_of: None,
            created_ms: 1,
            updated_ms: 1,
            completion: None,
        }
    }
    #[test]
    fn installed_reader_saves_its_center_and_source_target_together() {
        check_installed_source_target(
            Candidate {
                subject: "circled expression".into(),
                geometry: None,
                understanding: Understanding::Uncertain {
                    reason: "outline ambiguous".into(),
                },
            },
            None,
        );
    }

    #[test]
    fn installed_overview_keeps_the_recognized_subject_from_its_geometry_record() {
        let longest_subject = "é".repeat(256);
        for subject in ["24 / 6", longest_subject.as_str()] {
            check_installed_source_target(
                Candidate {
                    subject: subject.to_owned(),
                    geometry: Some(geometry::Geometry {
                        components: vec![vec![
                            [0, 0],
                            [1_000_000, 0],
                            [1_000_000, 1_000_000],
                            [0, 1_000_000],
                        ]],
                    }),
                    understanding: Understanding::Interpreted,
                },
                Some(subject),
            );
        }
    }

    fn check_installed_source_target(candidate: Candidate, expected_subject: Option<&str>) {
        let (_fixture, ledger, _, _) = fixture();
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 1).unwrap();
        let mut request = user(conversation);
        request.outcome = Outcome::Prepared;
        request.text = None;
        let capture = super::super::resident_capture::tests::capture_for(Uuid::new_v4());
        let id = Uuid::new_v4();
        ledger
            .prepare_resident_historical(
                Uuid::new_v4(),
                ledger
                    .expected(Namespace::Conversation, conversation)
                    .unwrap(),
                request.clone(),
                id,
                &capture,
            )
            .unwrap();
        let mut frame = ledger.target_source_frame(conversation, id).unwrap();
        let original = frame.source().record.clone();
        request.sources = vec![id];
        request.text = Some("What is the answer to this?".into());
        request.outcome = Outcome::Interpreted;
        let heads = ledger
            .expected(Namespace::Conversation, request.id)
            .unwrap();
        let operation = Uuid::new_v4();
        let center =
            crate::analysis::SelectionCenter::from_pixels(384.0, 256.0, 768, 1024).unwrap();
        let write = |frame: &mut SourceFrame| {
            ledger.advance_with_source_target(
                operation,
                heads.clone(),
                request.clone(),
                frame,
                &candidate,
                Some(center),
            )
        };
        ledger
            .store()
            .set_fault(crate::storage::Fault::BeforeCommit)
            .unwrap();
        assert!(write(&mut frame).is_err());
        assert_eq!(frame.source().record, original);
        assert_eq!(
            ledger
                .expected(Namespace::Conversation, request.id)
                .unwrap(),
            heads
        );
        assert_eq!(
            ledger
                .stored_resident_images(id)
                .unwrap()
                .evidence
                .reader_target,
            None
        );

        write(&mut frame).unwrap();
        frame.check(&ledger).unwrap();
        assert_ne!(frame.source().record.revision, original.revision);
        let context = ledger
            .context_range(
                conversation,
                None,
                &ContextBudget {
                    max_text_bytes: 4096,
                    max_turns: 4,
                    provider_token_limit: Some(4096),
                },
                |_| Ok(10),
            )
            .unwrap();
        let [target] = context.targets.as_slice() else {
            panic!("expected one saved source target");
        };
        assert_eq!(&target.target.source, frame.source());
        assert_eq!(target.target.candidate, candidate);
        let Record::ResidentCapture(origin) = &target.source else {
            panic!("expected pinned resident capture");
        };
        let expected = expected_subject.map(|_| target.reference.clone());
        assert_eq!(
            ledger
                .current_resident_writer_target(
                    conversation,
                    origin.before.document,
                    origin.before.page,
                )
                .unwrap(),
            expected
        );
        assert!(ledger
            .current_resident_writer_target(conversation, origin.before.document, Uuid::new_v4(),)
            .unwrap()
            .is_none());
        let operation = Uuid::new_v4();
        let resolved = crate::writer::target::CurrentRequest {
            operation,
            request: "Make a note about this marked source".into(),
            guidance: None,
        }
        .resolve(
            &ledger,
            conversation,
            origin.before.document,
            origin.before.page,
        );
        if let Some(expected) = expected {
            let crate::writer::target::Request::GenerateTarget {
                target,
                expected_root,
                request,
                operation: resolved_operation,
                ..
            } = resolved.unwrap();
            assert_eq!(target, expected);
            assert_eq!(
                expected_root,
                ledger
                    .expected(Namespace::Conversation, conversation)
                    .unwrap()
            );
            assert_eq!(resolved_operation, operation);
            assert_eq!(request, "Make a note about this marked source");
        } else {
            assert!(resolved.is_err());
        }
        ledger
            .stored_target_frame(conversation, &target.reference)
            .unwrap();
        let images = crate::conversation::receiving::images::resolve(&ledger, &context).unwrap();
        let associations = serde_json::to_value(images.associations).unwrap();
        let centers: Vec<_> = associations
            .as_array()
            .unwrap()
            .iter()
            .filter(|association| association.get("reader_target").is_some())
            .collect();
        assert_eq!(centers.len(), 1);
        assert_eq!(centers[0]["source"], id.to_string());
        assert_eq!(
            centers[0]["reader_target"]
                .get("subject")
                .and_then(|value| value.as_str()),
            expected_subject
        );
        assert_eq!(
            centers[0]["reader_target"]["normalized"],
            serde_json::json!([0.5, 0.25])
        );
    }

    #[test]
    fn inherited_target_retains_exact_original_outside_history_range_and_reopen() {
        let (_f, ledger, c, source) = fixture();
        let follow = user(c);
        ledger
            .append(
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, c).unwrap(),
                follow.clone(),
                vec![],
            )
            .unwrap();
        let paths = ledger.store.paths.clone();
        drop(ledger);
        let ledger = Ledger::new(Arc::new(Store::open(paths).unwrap()));
        let context = ledger
            .context_range(
                c,
                Some(TurnRange {
                    start: 1,
                    end_exclusive: 2,
                }),
                &ContextBudget {
                    max_text_bytes: 4096,
                    max_turns: 4,
                    provider_token_limit: Some(4096),
                },
                |_| Ok(10),
            )
            .unwrap();
        assert_eq!(context.turns.len(), 1);
        assert_eq!(context.turns[0].id, follow.id);
        assert!(context.sources.is_empty());
        assert_eq!(context.target_uses.len(), 1);
        assert_eq!(context.target_uses[0].action, Action::Inherit);
        assert_eq!(context.target_uses[0].about, None);
        assert_eq!(context.targets.len(), 1);
        assert_eq!(context.targets[0].target.source.record.record, source);
        let images = crate::conversation::receiving::images::resolve(&ledger, &context).unwrap();
        assert_eq!(images.attachments.len(), 1);
        let a = serde_json::to_value(images.associations).unwrap();
        assert_eq!(a[0]["source"], context.targets[0].target.id.to_string());
        let mut missing = context;
        missing.targets.clear();
        assert!(crate::conversation::receiving::images::resolve(&ledger, &missing).is_err());
    }
    #[test]
    fn explicit_subject_correction_retargets_and_cli_replay_preserve_original_history() {
        let (f, ledger, c, source) = fixture();
        let budget = ContextBudget {
            max_text_bytes: 8192,
            max_turns: 16,
            provider_token_limit: Some(8192),
        };
        let initial = ledger.context(c, &budget, |_| Ok(1)).unwrap().targets[0].clone();
        let typed = user(c);
        let op = Uuid::new_v4();
        let expected = ledger.expected(Namespace::Conversation, c).unwrap();
        let correction = TargetChange::CorrectSubject {
            target: initial.reference.clone(),
            subject: "Corrected subject".into(),
        };
        let ack = ledger
            .change_target(op, expected.clone(), typed.clone(), &correction)
            .unwrap();
        assert_eq!(
            ledger
                .change_target(op, expected, typed, &correction)
                .unwrap()
                .historical,
            ack.historical
        );
        let context = ledger.context(c, &budget, |_| Ok(1)).unwrap();
        assert_eq!(context.targets.len(), 2);
        let corrected = context
            .targets
            .iter()
            .find(|v| v.target.candidate.subject == "Corrected subject")
            .unwrap();
        assert_eq!(initial.target.id, corrected.target.id);
        assert_ne!(initial.reference.revision, corrected.reference.revision);
        assert_eq!(corrected.target.previous, Some(initial.reference.clone()));
        let follow = user(c);
        ledger
            .append(
                Uuid::new_v4(),
                ledger.expected(Namespace::Conversation, c).unwrap(),
                follow.clone(),
                vec![],
            )
            .unwrap();
        let context = ledger.context(c, &budget, |_| Ok(1)).unwrap();
        let inherited = context
            .target_uses
            .iter()
            .find(|u| u.turn == follow.id)
            .unwrap();
        assert_eq!(inherited.target, Some(corrected.reference.clone()));
        assert!(inherited.about.is_none());
        let config = crate::config::Config {
            paths: ledger.store().paths.clone(),
            ..Default::default()
        };
        let config_path = f.0.join("config.json");
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let change = TargetChange::Retarget {
            source,
            candidate: Candidate {
                subject: "Different region".into(),
                geometry: Some(geometry::Geometry {
                    components: vec![vec![
                        [0, 0],
                        [500_000, 0],
                        [500_000, 1_000_000],
                        [0, 1_000_000],
                    ]],
                }),
                understanding: Understanding::Interpreted,
            },
        };
        let request = serde_json::json!({"operation":Uuid::new_v4(),"conversation":c,"user":Uuid::new_v4(),"expected_root":ledger.expected(Namespace::Conversation,c).unwrap(),"text":"Use this different bounded region", "change":change});
        let bytes = serde_json::to_vec(&request).unwrap();
        drop(ledger);
        let execute = || {
            serde_json::to_value(super::super::target_command::run(
                Some(config_path.clone()),
                std::path::Path::new("-"),
                &mut &bytes[..],
            ))
            .unwrap()
        };
        let report = execute();
        assert_eq!(report["status"], "acknowledged");
        assert_eq!(report, execute());
        let ledger = Ledger::new(Arc::new(Store::open(config.paths).unwrap()));
        let context = ledger.context(c, &budget, |_| Ok(1)).unwrap();
        let retarget = context
            .targets
            .iter()
            .find(|v| v.target.candidate.subject == "Different region")
            .unwrap();
        assert_ne!(retarget.target.id, initial.target.id);
        assert_eq!(retarget.target.source, initial.target.source);
        assert_eq!(
            context
                .target_uses
                .iter()
                .filter(|u| u.action == Action::CorrectSubject)
                .count(),
            1
        );
        assert_eq!(
            context
                .target_uses
                .iter()
                .filter(|u| u.action == Action::Retarget)
                .count(),
            1
        );
        assert!(ledger
            .store()
            .snapshot_heads_matching(&[Namespace::Handwriting], 16, |_| true)
            .unwrap()
            .is_empty());
        let images = crate::conversation::receiving::images::resolve(&ledger, &context).unwrap();
        assert_eq!(images.attachments.len(), 1);
        assert_eq!(
            images.associations.len(),
            3,
            "original capture and two target scopes retain associations"
        );
    }
    #[test]
    fn target_frame_pins_raw_loaded_config_and_ready_capabilities_across_own_root_updates() {
        let (f, ledger, c, source) = fixture();
        let config = crate::config::Config {
            paths: ledger.store().paths.clone(),
            ..Default::default()
        };
        let path = f.0.join("config.json");
        let bytes = serde_json::to_vec(&config).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        drop(ledger);
        let loaded = crate::config::Config::load(Some(path.clone())).unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&config).unwrap(),
            "runtime file pins never change serialized configuration"
        );
        let ledger = Ledger::new(Arc::new(
            Store::open_source_target_startup(&loaded).unwrap(),
        ));
        let frame = ledger.target_source_frame(c, source).unwrap();
        frame.check_current().unwrap();
        let mut changed = bytes.clone();
        changed.extend_from_slice(b" ");
        std::fs::write(&path, &changed).unwrap();
        assert!(
            frame.check_current().is_err(),
            "even semantically equal changed Config bytes must lose original correspondence"
        );
        std::fs::write(&path, &bytes).unwrap();
        frame.check_current().unwrap();
        let capabilities = loaded.paths.data.join("capabilities.json");
        let original = std::fs::read(&capabilities).unwrap();
        let mut changed = original.clone();
        changed.push(b' ');
        std::fs::write(&capabilities, &changed).unwrap();
        assert!(
            frame.check_current().is_err(),
            "ready capability mutation must not be ignored until reopen"
        );
        std::fs::write(&capabilities, original).unwrap();
        frame.check_current().unwrap();
    }
}

impl Ledger {
    fn stored_source_target(
        &self,
        conversation: Uuid,
        reference: &RecordRef,
    ) -> Result<SourceTarget> {
        ensure!(
            reference.namespace == Namespace::Source,
            "target namespace differs"
        );
        let envelope = self
            .store
            .value(Namespace::Source, reference.record)?
            .context("stored target absent")?;
        ensure!(
            envelope.revision_id == reference.revision
                && self.store.notebook_revision_reference(&envelope)? == reference.object,
            "stored target changed"
        );
        let Record::SourceTarget(target) = Self::decode(&envelope)? else {
            bail!("not a stored target")
        };
        ensure!(target.conversation == conversation, "foreign stored target");
        Ok(*target)
    }

    pub(crate) fn stored_target_frame(
        &self,
        conversation: Uuid,
        reference: &RecordRef,
    ) -> Result<(SourceFrame, SourceTarget)> {
        let target = self.stored_source_target(conversation, reference)?;
        let frame = self.target_source_frame(conversation, target.source.record.record)?;
        ensure!(
            frame.source == target.source,
            "stored target original changed"
        );
        Ok((frame, target))
    }
    pub(crate) fn target_document(&self, target: &SourceTarget) -> Result<Uuid> {
        let encoded = self.store.read_object(&target.source.record.object)?;
        let e: Envelope = serde_json::from_slice(&encoded)?;
        let record = Self::decode(&e)?;
        Ok(target
            .source
            .validate_record(&record, target.conversation)?
            .unwrap_or_else(|| {
                Uuid::new_v5(
                    &target.conversation,
                    format!("historical-unbound:{}", target.source.record.record).as_bytes(),
                )
            }))
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TargetChange {
    CorrectSubject { target: RecordRef, subject: String },
    Retarget { source: Uuid, candidate: Candidate },
}
impl Ledger {
    /// Explicit historical intent under root CAS. This never confirms handwriting,
    /// infers a region from prose or grants a native output lease.
    pub fn change_target(
        &self,
        operation: Uuid,
        expected: ExpectedHeads,
        mut user: Turn,
        change: &TargetChange,
    ) -> Result<WriteResult> {
        let fingerprint = Self::fingerprint(&("explicit-target-change-v1", &user, change))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(prior);
        }
        Self::validate_turn(&user)?;
        ensure!(
            user.role == Role::User
                && user.mode == Mode::Reader
                && user.outcome == Outcome::Interpreted
                && user.sequence == 0
                && user.sources.is_empty(),
            "invalid explicit target user"
        );
        let (frame, old, candidate, action) = match change {
            TargetChange::CorrectSubject { target, subject } => {
                let (frame, old) = self.stored_target_frame(user.conversation, target)?;
                let mut candidate = old.candidate.clone();
                candidate.subject = subject.clone();
                candidate.validate()?;
                (
                    frame,
                    Some((target.clone(), old)),
                    candidate,
                    Action::CorrectSubject,
                )
            }
            TargetChange::Retarget { source, candidate } => {
                candidate.validate()?;
                (
                    self.target_source_frame(user.conversation, *source)?,
                    None,
                    candidate.clone(),
                    Action::Retarget,
                )
            }
        };
        ensure!(
            frame.expected_root() == expected,
            "explicit target root changed"
        );
        frame.check(self)?;
        let (_, mut root) = self.live_root(user.conversation, Some(&expected))?;
        user.sequence = root.next_sequence;
        if user.created_ms == 0 {
            user.created_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
                .try_into()?;
            user.updated_ms = user.created_ms;
        }
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("target sequence exhausted")?;
        user.updated_ms = user.updated_ms.max(root.updated_ms);
        root.updated_ms = user.updated_ms;
        let id = old
            .as_ref()
            .map(|(_, t)| t.id)
            .unwrap_or_else(|| Uuid::new_v5(&user.id, b"urn:rmb:source-target:v1"));
        let target = SourceTarget {
            id,
            conversation: user.conversation,
            introduced_by: user.id,
            source: frame.source.clone(),
            candidate,
            previous: old.as_ref().map(|(r, _)| r.clone()),
        };
        let parents = old
            .as_ref()
            .map(|(r, _)| ExpectedHeads(BTreeSet::from([r.revision])))
            .unwrap_or_default();
        let about = target.candidate.about()?;
        let target = self.envelope(
            Namespace::Source,
            id,
            parents,
            Record::SourceTarget(Box::new(target.clone())),
            target.media(),
        )?;
        let pin = |e: &Envelope| -> Result<RecordRef> {
            let bytes = serde_json::to_vec(e)?;
            Ok(RecordRef {
                namespace: e.namespace,
                record: e.record_id,
                revision: e.revision_id,
                object: ObjectRef {
                    sha256: digest(&bytes),
                    bytes: bytes.len() as u64,
                },
            })
        };
        let used = TargetUse {
            id: TargetUse::id(user.id),
            conversation: user.conversation,
            turn: user.id,
            action,
            target: Some(pin(&target)?),
            about: Some(about),
            inline_body_sha256: None,
        };
        let used = self.envelope(
            Namespace::Conversation,
            used.id,
            ExpectedHeads::default(),
            Record::TargetUse(Box::new(used)),
            vec![],
        )?;
        let state = TargetState {
            id: TargetState::id(user.conversation),
            conversation: user.conversation,
            use_reference: pin(&used)?,
        };
        let state = self.envelope(
            Namespace::Conversation,
            state.id,
            frame.state_heads.clone(),
            Record::TargetState(Box::new(state)),
            vec![],
        )?;
        let user = self.envelope(
            Namespace::Conversation,
            user.id,
            ExpectedHeads::default(),
            Record::Turn(user),
            vec![],
        )?;
        let binding = root.binding;
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            expected,
            Record::Root(root),
            vec![],
        )?;
        self.publish(
            operation,
            fingerprint,
            root,
            vec![user, target, used, state],
            binding,
        )
    }
}

#[cfg(all(target_os = "linux", feature = "installed-resident-output"))]
mod region_export;

impl Ledger {
    /// Reopen/refine/export resolves the exact saved target and original capture,
    /// reuses its established validators and verifies the deterministic mask.
    /// No source head, active target, recognizer or model call substitutes for it.
    pub(crate) fn regional_source_proof(
        &self,
        region: &RegionSource,
        progress: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        region.validate()?;
        ensure!(
            self.store.ordinary_reader_family()?
                && self.store.source_target_generation()?.is_some(),
            "regional receiving requires ordinary target family"
        );
        progress()?;
        let used = TargetUse {
            id: TargetUse::id(region.turn),
            conversation: region.conversation,
            turn: region.turn,
            action: Action::Inherit,
            target: Some(region.target.clone()),
            about: None,
            inline_body_sha256: None,
        };
        let mut views = self.target_views(&[used])?;
        let view = views.pop().context("regional exact target absent")?;
        ensure!(
            view.target.conversation == region.conversation
                && view.target.source.original == region.original
                && view.target.source.dimensions == region.dimensions
                && view.target.candidate.about()? == region.about
                && matches!(
                    view.target.candidate.understanding,
                    Understanding::Interpreted
                ),
            "regional saved target correspondence differs"
        );
        if let Some(exact) = &region.installed_region {
            let Record::ResidentCapture(capture) = &view.source else {
                bail!("installed export region requires a resident original");
            };
            ensure!(
                exact.document == capture.before.document
                    && exact.page == capture.before.page
                    && Some(&exact.geometry) == view.target.candidate.geometry.as_ref(),
                "installed export region differs from its historical target"
            );
        }
        // One cumulative proof allowance, charged before each actual media read.
        let mut remaining = 64 * 1024 * 1024u64;
        let mut objects = 0usize;
        let (original, derived) = {
            let mut load = |media: &Media| -> Result<Vec<u8>> {
                progress()?;
                objects = objects
                    .checked_add(1)
                    .context("regional media work overflow")?;
                remaining = remaining
                    .checked_sub(media.bytes)
                    .context("regional media proof byte bound")?;
                ensure!(objects <= 16, "regional media proof object bound");
                self.store.read_object(&ObjectRef {
                    sha256: media.sha256.clone(),
                    bytes: media.bytes,
                })
            };
            let original = match view.source {
                Record::Source(_) => load(&region.original)?,
                Record::ResidentCapture(v) => {
                    Self::resident_images_from_evidence(*v, &mut load)?.original_png
                }
                Record::DevelopmentCapture(v) => {
                    Self::development_images_from_evidence(*v, &mut load)?.original_png
                }
                Record::Capture(v) => {
                    Self::sdk_images_from_evidence(*v, &mut load)?
                        .native_parent
                        .bytes
                }
                Record::LegacyCapture(v) => {
                    let bytes = Self::legacy_images_from_evidence(v, &mut load)?;
                    bytes.parent.unwrap_or_else(|| bytes.images[0].clone())
                }
                _ => bail!("regional original is not a capture"),
            };
            let derived = load(&region.derived)?;
            (original, derived)
        };
        let geometry = view
            .target
            .candidate
            .geometry
            .as_ref()
            .context("regional bounded target absent")?;
        let expected = if let Some(exact) = &region.installed_region {
            let (masked, coverage) =
                geometry.mask_with_coverage(&original, region.dimensions, progress)?;
            ensure!(
                exact.coverage_sha256 == coverage,
                "saved region raster coverage differs"
            );
            masked
        } else {
            geometry.mask_png(&original, region.dimensions, progress)?
        };
        ensure!(
            expected == derived,
            "saved region mask/recipe differs from original target"
        );
        let admitted = crate::retrieval::notebook::admit_target(1, derived, &mut |_| progress())?;
        ensure!(
            admitted.source_sha256() == region.normalized.document.source_sha256,
            "regional normalization input identity differs"
        );
        progress()?;
        Ok(())
    }
}
