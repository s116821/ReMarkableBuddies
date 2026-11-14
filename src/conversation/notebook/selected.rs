//! Local selected notebook transitions. Only freshly observed commits carry a cursor.
use super::{
    persistence::{encoded_revision, finished_stage, prepared_stage},
    *,
};
use crate::conversation::{
    DocumentOwnership, IntentRecordRef, IntentSelectionEvidence, SelectedAdmission,
    SelectedDomainProjection,
};
use crate::storage::selection::{
    SelectedReadBudget, SelectedSnapshot, SelectionPublication, SelectionToken,
};

pub(crate) struct NotebookSelection {
    pub expected: IntentSelectionEvidence,
    pub raw_head_sha256: String,
    pub root: IntentRecordRef,
    pub prior: Option<(IntentRecordRef, IntentRecordRef)>,
}

pub(crate) struct NotebookStart {
    pub plan: NotebookPlan,
    pub archive: ArchivedNormalizedSource,
    pub canonical: Vec<u8>,
}

/// The original phase allowance and a pure late cancellation/time check.
/// Consuming a cursor never creates a replacement budget or renews its deadline.
pub(crate) struct NotebookPublication<'a> {
    pub reads: &'a mut SelectedReadBudget,
    pub check: &'a mut dyn FnMut() -> Result<()>,
    pub unobserved: &'a mut bool,
}

impl NotebookPublication<'_> {
    fn commit(
        &mut self,
        store: &Store,
        expected: &SelectionToken,
        operation: Uuid,
        records: Vec<Envelope>,
        media: BTreeMap<String, Vec<u8>>,
    ) -> Result<SelectionPublication> {
        *self.unobserved = true;
        let publication = store.commit_selected_guarded(
            expected,
            operation,
            records,
            media,
            self.reads,
            &[],
            self.check,
        )?;
        *self.unobserved = publication.replayed;
        Ok(publication)
    }
}

pub(crate) struct SelectedNotebookCursor {
    token: SelectionToken,
    root: Envelope,
    plan: NotebookPlan,
    plan_revision: String,
    results: Vec<Envelope>,
}

pub(crate) struct SelectedNotebookStage {
    cursor: SelectedNotebookCursor,
    envelope: Envelope,
}

pub(crate) fn initial_snapshot(
    admission: &SelectedAdmission,
    store: &Store,
    selection: &NotebookSelection,
    reads: &mut SelectedReadBudget,
) -> Result<(SelectedSnapshot, SelectedDomainProjection)> {
    ensure!(
        !store.ordinary_reader_family()?,
        "selected notebook requires format 5"
    );
    let read = store
        .selected_snapshot_with_budget(admission.scope(), MAX_ITEMS, reads)?
        .context("notebook selected scope absent")?;
    ensure!(
        IntentSelectionEvidence::from_token(&read.snapshot.token) == selection.expected
            && read
                .accepted_history
                .first()
                .is_some_and(|head| head.sha256 == selection.raw_head_sha256),
        "notebook selected admission changed"
    );
    let projection = SelectedDomainProjection::from_snapshot(&read.snapshot)?;
    ensure!(
        projection.current_reference(Namespace::Conversation, selection.root.record_id)?
            == selection.root
            && matches!(
                projection.exact_reference(&selection.root)?,
                Record::Root(_)
            ),
        "notebook selected root changed"
    );
    Ok((read.snapshot, projection))
}

pub(crate) fn context_and_images(
    snapshot: &mut SelectedSnapshot,
    projection: &SelectedDomainProjection,
    conversation: Uuid,
    range: Option<TurnRange>,
    reads: &mut SelectedReadBudget,
) -> Result<(Vec<Turn>, crate::conversation::receiving::images::Images)> {
    let context = Ledger::context_from_records(
        projection.inspect(conversation)?,
        range,
        &ContextBudget {
            max_text_bytes: 16 * 1024,
            max_turns: 256,
            provider_token_limit: Some(16 * 1024),
        },
        |turns| {
            Ok(turns
                .iter()
                .map(|turn| turn.text.as_ref().map_or(0, String::len))
                .sum())
        },
        |media| {
            snapshot
                .transaction
                .selected
                .media
                .iter()
                .any(|object| object.sha256 == media.sha256 && object.bytes == media.bytes)
        },
    )?;
    let images = crate::conversation::receiving::images::resolve_with(&context, |reference| {
        read_selected_media(snapshot, reference, reads)
    })?;
    Ok((context.turns, images))
}

pub(crate) fn read_selected_media(
    snapshot: &mut SelectedSnapshot,
    reference: &ObjectRef,
    reads: &mut SelectedReadBudget,
) -> Result<Vec<u8>> {
    ensure!(
        snapshot.transaction.selected.media.contains(reference),
        "notebook selected media omitted or missing"
    );
    let source = snapshot
        .media
        .get_mut(&reference.sha256)
        .context("notebook selected media handle absent")?;
    reads.charge(0, 1, 0, reference.bytes)?;
    let bytes = source.chunk(0, reference.bytes.try_into()?)?;
    ensure!(
        digest(&bytes) == reference.sha256,
        "notebook selected media integrity mismatch"
    );
    Ok(bytes)
}

impl SelectedNotebookCursor {
    pub(crate) fn begin(
        admission: &SelectedAdmission,
        store: &Store,
        selection: &NotebookSelection,
        start: NotebookStart,
        mut publication: NotebookPublication<'_>,
    ) -> Result<Option<Self>> {
        let NotebookStart {
            mut plan,
            archive,
            canonical,
        } = start;
        let (snapshot, projection) =
            initial_snapshot(admission, store, selection, publication.reads)?;
        let source = archive.validate_bytes(&canonical)?;
        plan.validate_source(&archive, &source)?;
        let mut media = BTreeMap::new();
        ensure!(
            plan.conversation == selection.root.record_id
                && crate::memory::catalog::document_key(archive.document)?
                    == admission.scope().key_sha256,
            "selected notebook document ownership mismatch"
        );
        let ownership = projection.document_ownership()?;
        ensure!(
            matches!(
                ownership.get(&plan.conversation),
                Some(DocumentOwnership::DeferredMissing)
            ) || ownership.get(&plan.conversation)
                == Some(&DocumentOwnership::Document(archive.document)),
            "notebook conversation has another document owner"
        );
        let (_, Some(Record::Root(root))) = projection
            .head(Namespace::Conversation, plan.conversation)
            .context("selected notebook root absent")?
        else {
            bail!("invalid selected notebook root");
        };
        let ledger = Ledger::new(admission.writer_store().clone());
        let user_id = crate::writer::stage(plan.operation, "user");
        let mut records = Vec::new();
        plan.source_revision = match (&selection.prior, &plan.prior) {
            (None, None) => {
                ensure!(
                    archive.turn == user_id,
                    "selected notebook archive origin changed"
                );
                let envelope = ledger.envelope(
                    Namespace::Source,
                    archive.id,
                    ExpectedHeads::default(),
                    Record::ArchivedNormalizedSource(Box::new(archive.clone())),
                    vec![archive.media.clone()],
                )?;
                let revision = encoded_revision(&envelope)?;
                records.push(envelope);
                media.insert(archive.media.sha256.clone(), canonical);
                revision
            }
            (Some((source, draft)), Some((id, revision, text))) => {
                ensure!(
                    source.namespace == Namespace::Source
                        && draft.namespace == Namespace::Conversation
                        && source.record_id == archive.id
                        && draft.record_id == *id
                        && draft.object.sha256 == *revision
                        && projection.current_reference(source.namespace, source.record_id)?
                            == *source
                        && projection.current_reference(draft.namespace, draft.record_id)?
                            == *draft
                        && matches!(projection.exact_reference(source)?, Record::ArchivedNormalizedSource(value) if **value == archive)
                        && matches!(projection.exact_reference(draft)?, Record::Turn(value)
                            if value.conversation == plan.conversation
                                && value.mode == Mode::Writer && value.role == Role::Assistant
                                && value.outcome == Outcome::Generated
                                && value.sources == [archive.id]
                                && value.text.as_ref().is_some_and(|body| digest(body.as_bytes()) == *text)),
                    "selected notebook refinement lineage changed"
                );
                source.object.sha256.clone()
            }
            _ => bail!("selected notebook refinement input mismatch"),
        };
        let plan_envelope = ledger.envelope(
            Namespace::Conversation,
            plan.id,
            ExpectedHeads::default(),
            Record::NotebookPlan(Box::new(plan.clone())),
            vec![],
        )?;
        let plan_revision = encoded_revision(&plan_envelope)?;
        records.push(plan_envelope);
        let (root, user) = append_turn(
            &ledger,
            selection.root.revision_id,
            root.clone(),
            &plan,
            Role::User,
            user_id,
            plan.request.clone(),
        )?;
        records.extend([root.clone(), user]);
        let publication =
            publication.commit(store, &snapshot.token, plan.operation, records, media)?;
        Ok((!publication.replayed).then_some(Self {
            token: publication.token,
            root,
            plan,
            plan_revision,
            results: Vec::new(),
        }))
    }

    pub(crate) fn prepare(
        mut self,
        admission: &SelectedAdmission,
        store: &Store,
        kind: NotebookStageKind,
        ordinal: usize,
        input_digest: String,
        mut publication: NotebookPublication<'_>,
    ) -> Result<Option<SelectedNotebookStage>> {
        let stage = prepared_stage(
            &self.plan,
            &self.plan_revision,
            &self.results,
            kind,
            ordinal,
            input_digest,
        )?;
        let operation = stage.transition(false);
        let ledger = Ledger::new(admission.writer_store().clone());
        let envelope = ledger.envelope(
            Namespace::Conversation,
            stage.id,
            ExpectedHeads::default(),
            Record::NotebookStage(Box::new(stage)),
            vec![],
        )?;
        let publication = publication.commit(
            store,
            &self.token,
            operation,
            vec![envelope.clone()],
            BTreeMap::new(),
        )?;
        self.token = publication.token;
        Ok((!publication.replayed).then_some(SelectedNotebookStage {
            cursor: self,
            envelope,
        }))
    }

    pub(crate) fn map_outputs(&self) -> Result<Vec<crate::writer::notebook::NotebookMapOutput>> {
        super::persistence::map_outputs(&self.plan, &self.results)
    }

    pub(crate) fn token(&self) -> &SelectionToken {
        &self.token
    }

    pub(crate) fn finalize(
        self,
        admission: &SelectedAdmission,
        store: &Store,
        mut publication: NotebookPublication<'_>,
    ) -> Result<Option<Uuid>> {
        let reducer = self
            .results
            .last()
            .context("selected notebook reducer absent")?;
        let Record::NotebookStage(reducer) = Ledger::decode(reducer)? else {
            bail!("selected notebook reducer absent");
        };
        ensure!(
            self.results.len() == self.plan.chunks.len() + 1
                && reducer.kind == NotebookStageKind::Reduce
                && reducer.outcome == Some(NotebookStageOutcome::Generated),
            "selected notebook plan incomplete"
        );
        let text = reducer
            .output
            .context("selected notebook final text absent")?;
        crate::writer::parse_draft(&text)?;
        let Record::Root(root) = Ledger::decode(&self.root)? else {
            bail!("selected notebook root absent");
        };
        let ledger = Ledger::new(admission.writer_store().clone());
        let id = crate::writer::stage(self.plan.operation, "draft");
        let (root, draft) = append_turn(
            &ledger,
            self.root.revision_id,
            root,
            &self.plan,
            Role::Assistant,
            id,
            text,
        )?;
        let operation = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "urn:rmb:writer-notebook-transition:v1:{}:finalize",
                self.plan.id
            )
            .as_bytes(),
        );
        let publication = publication.commit(
            store,
            &self.token,
            operation,
            vec![root, draft],
            BTreeMap::new(),
        )?;
        Ok((!publication.replayed).then_some(id))
    }
}

impl SelectedNotebookStage {
    pub(crate) fn token(&self) -> &SelectionToken {
        self.cursor.token()
    }

    pub(crate) fn finish(
        self,
        admission: &SelectedAdmission,
        store: &Store,
        result: NotebookResult,
        mut publication: NotebookPublication<'_>,
    ) -> Result<Option<(SelectedNotebookCursor, bool)>> {
        let Self {
            mut cursor,
            envelope: prepared,
        } = self;
        let stage = finished_stage(&prepared, result)?;
        let operation = stage.transition(true);
        let generated = stage.outcome == Some(NotebookStageOutcome::Generated);
        let ledger = Ledger::new(admission.writer_store().clone());
        let envelope = ledger.envelope(
            Namespace::Conversation,
            stage.id,
            ExpectedHeads(BTreeSet::from([prepared.revision_id])),
            Record::NotebookStage(stage),
            vec![],
        )?;
        let publication = publication.commit(
            store,
            &cursor.token,
            operation,
            vec![envelope.clone()],
            BTreeMap::new(),
        )?;
        cursor.token = publication.token;
        cursor.results.push(envelope);
        Ok((!publication.replayed).then_some((cursor, generated)))
    }
}

fn append_turn(
    ledger: &Ledger,
    root_revision: Uuid,
    mut root: Root,
    plan: &NotebookPlan,
    role: Role,
    id: Uuid,
    text: String,
) -> Result<(Envelope, Envelope)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    let user = role == Role::User;
    let turn = Turn {
        id,
        conversation: root.id,
        exchange: plan.operation,
        sequence: root.next_sequence,
        role,
        mode: Mode::Writer,
        outcome: if user {
            Outcome::Interpreted
        } else {
            Outcome::Generated
        },
        text: Some(text),
        sources: vec![plan.archive_id],
        correction_of: if user {
            plan.prior.as_ref().map(|prior| prior.0)
        } else {
            None
        },
        created_ms: now,
        updated_ms: now,
        completion: None,
    };
    Ledger::validate_turn(&turn)?;
    root.next_sequence = root
        .next_sequence
        .checked_add(1)
        .context("notebook chronology exhausted")?;
    root.updated_ms = now;
    let root = ledger.envelope(
        Namespace::Conversation,
        root.id,
        ExpectedHeads(BTreeSet::from([root_revision])),
        Record::Root(root),
        vec![],
    )?;
    let turn = ledger.envelope(
        Namespace::Conversation,
        id,
        ExpectedHeads::default(),
        Record::Turn(turn),
        vec![],
    )?;
    Ok((root, turn))
}

#[cfg(test)]
pub(crate) fn fixture_resident(
    document: Uuid,
    conversation: Uuid,
    turn: Uuid,
    evidence: Uuid,
) -> Result<crate::conversation::resident_capture::PreparedResidentCapture> {
    crate::conversation::resident_capture::tests::capture_with_pixels(
        document,
        image::RgbaImage::from_fn(12, 20, |x, y| {
            image::Rgba([x as u8 * 10, y as u8 * 10, 41, 255])
        }),
    )
    .prepare_history(conversation, turn, evidence)
}
