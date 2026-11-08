//! One private retained-frame resolver. Serialized references alone admit no use.
use super::{target::*, *};
use crate::storage::{digest, ObjectRef};
use std::sync::{Arc, Weak};
pub(crate) struct SourceFrame {
    store: Weak<Store>,
    generation: Uuid,
    state_heads: ExpectedHeads,
    pub(super) root: ExpectedHeads,
    pub(super) source: RetainedSourceRef,
    pub(super) original: Vec<u8>,
    pub(super) conversation: Uuid,
}
fn descriptor(reference: RecordRef, record: &Record) -> Result<(RetainedSourceRef, Uuid)> {
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
        let heads = store
            .heads(Namespace::Conversation, self.conversation)?
            .iter()
            .map(|e| e.revision_id)
            .collect::<BTreeSet<_>>();
        ensure!(heads == self.root.0, "target root changed");
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
    pub(crate) fn expected_root(&self) -> ExpectedHeads {
        self.root.clone()
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
        let frame = SourceFrame {
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
        let id = Uuid::new_v5(&user.id, b"urn:rmb:source-target:v1");
        let value = SourceTarget {
            id,
            conversation: user.conversation,
            introduced_by: user.id,
            source: frame.source.clone(),
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
        expected_root: ExpectedHeads,
        expected_turn: ExpectedHeads,
        turn: Turn,
        frame: &SourceFrame,
        candidate: &Candidate,
    ) -> Result<WriteResult> {
        self.advance_fact_target(
            operation,
            expected_root,
            expected_turn,
            turn,
            None,
            Some((frame, candidate)),
        )
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
