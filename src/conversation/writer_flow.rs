//! Invocation-bound host Writer intent/selection, never native authority.
use super::*;

pub(crate) struct WriterFlowGrant {
    operation: Uuid,
    fingerprint: String,
    conversation: Uuid,
    correlation: Uuid,
    effect: Uuid,
    draft: Uuid,
    store: usize,
    pub(crate) root: ExpectedHeads,
}
pub(crate) struct WriterFlowAdmission {
    pub(crate) conversation: Uuid,
    pub(crate) operation: Uuid,
    pub(crate) fingerprint: String,
    pub(crate) correlation: Uuid,
    pub(crate) effect: Uuid,
    pub(crate) draft: Uuid,
    pub(crate) expected: ExpectedHeads,
}

pub(crate) struct WriterSelection {
    pub(crate) root: ExpectedHeads,
    draft: Envelope,
    source: Envelope,
}
impl WriterSelection {
    pub(crate) fn published(root: &Envelope, draft: &Envelope, source: &Envelope) -> Result<Self> {
        let value = Self {
            root: ExpectedHeads(BTreeSet::from([root.revision_id])),
            draft: draft.clone(),
            source: source.clone(),
        };
        value.values()?;
        Ok(value)
    }
    pub(crate) fn values(&self) -> Result<(Turn, NormalizedTextSource)> {
        let Record::Turn(draft) = Ledger::decode(&self.draft)? else {
            bail!("not a Writer draft")
        };
        let Record::NormalizedText(source) = Ledger::decode(&self.source)? else {
            bail!("not a text source")
        };
        ensure!(
            draft.mode == Mode::Writer
                && draft.role == Role::Assistant
                && draft.outcome == Outcome::Generated
                && draft.conversation == source.conversation
                && draft.sources == vec![source.id],
            "invalid Writer selection"
        );
        crate::writer::parse_draft(draft.text.as_deref().context("draft text missing")?)?;
        Ok((draft, *source))
    }
}

pub(crate) struct WriterExportInput {
    grant: WriterFlowGrant,
    pub(crate) selection: WriterSelection,
}
impl WriterExportInput {
    pub(crate) fn new(grant: WriterFlowGrant, selection: WriterSelection) -> Self {
        Self { grant, selection }
    }
    pub(crate) fn note(&self) -> Result<crate::note_export::PreparedNote> {
        let (draft, source) = self.selection.values()?;
        crate::writer::note_from_draft(&draft, &source, self.grant.correlation, self.grant.effect)
    }
    pub(crate) fn matches(&self, note: &crate::note_export::PreparedNote) -> Result<bool> {
        let expected = self.note()?;
        Ok(serde_json::to_vec(&expected)? == serde_json::to_vec(note)?)
    }
    pub(crate) fn fingerprint(
        &self,
        ledger: &Ledger,
        association: &ExportAssociation,
    ) -> Result<String> {
        let (draft, _) = self.selection.values()?;
        ensure!(
            self.grant.store == ledger.store().as_ref() as *const Store as usize
                && self.grant.conversation == association.conversation
                && self.grant.correlation == association.id
                && self.grant.effect == association.attempt.as_ref().context("attempt missing")?.id
                && self.grant.draft == draft.id,
            "foreign Writer flow grant"
        );
        let note = self.note()?;
        ensure!(
            association.source_scope == note.source_scope
                && association.source_revision == note.source_revision,
            "export does not match selected Writer provenance"
        );
        let receipt = ledger
            .store
            .value(
                Namespace::Conversation,
                Ledger::receipt_id(self.grant.operation),
            )?
            .context("outer intent missing")?;
        ensure!(
            matches!(Ledger::decode(&receipt)?, Record::Receipt(r)
            if r.fingerprint == self.grant.fingerprint && r.acknowledgment.operation == self.grant.operation),
            "outer intent changed"
        );
        Ledger::fingerprint(&(
            "writer-export-v1",
            association,
            self.grant.operation,
            &self.selection.root.0,
            self.selection.draft.revision_id,
            self.selection.source.revision_id,
        ))
    }
    pub(crate) fn guard_records(&self, ledger: &Ledger) -> Result<Vec<Envelope>> {
        self.selection.values()?;
        Ok([&self.selection.draft, &self.selection.source]
            .into_iter()
            .map(|old| {
                let mut guard = old.clone();
                guard.parents = BTreeSet::from([old.revision_id]);
                guard.revision_id = Uuid::new_v4();
                guard.operation_id = Uuid::new_v4();
                guard.actor_id = ledger.store.actor_id;
                guard
            })
            .collect())
    }
}

pub(crate) struct WriterExportGuard(Vec<(Namespace, Uuid, Uuid)>);
impl WriterExportGuard {
    pub(crate) fn published(root: &Envelope, records: &[Envelope]) -> Self {
        Self(
            std::iter::once(root)
                .chain(records)
                .map(|e| (e.namespace, e.record_id, e.revision_id))
                .collect(),
        )
    }
    pub(crate) fn current(&self, ledger: &Ledger) -> Result<bool> {
        ledger.complete_store()?;
        let heads = ledger.store.snapshot_heads_matching(
            &[
                Namespace::Conversation,
                Namespace::Source,
                Namespace::ExportAssociation,
            ],
            MAX_ITEMS,
            |e| {
                self.0
                    .iter()
                    .any(|(namespace, id, _)| e.namespace == *namespace && e.record_id == *id)
            },
        )?;
        Ok(self.0.iter().all(|(namespace, id, revision)| {
            let values: Vec<_> = heads
                .iter()
                .filter(|e| e.namespace == *namespace && e.record_id == *id)
                .collect();
            values.len() == 1 && values[0].revision_id == *revision && values[0].kind == Kind::Value
        }))
    }
}

impl Ledger {
    pub(crate) fn admit_writer_flow(
        &self,
        request: WriterFlowAdmission,
    ) -> Result<Option<WriterFlowGrant>> {
        let WriterFlowAdmission {
            conversation,
            operation,
            fingerprint,
            correlation,
            effect,
            draft,
            expected,
        } = request;
        if self.replay(operation, &fingerprint)?.is_some() {
            return Ok(None);
        }
        let (_, root) = self.live_root(conversation, Some(&expected))?;
        let binding = root.binding;
        let envelope = self.envelope(
            Namespace::Conversation,
            conversation,
            expected,
            Record::Root(root),
            vec![],
        )?;
        let current = ExpectedHeads(BTreeSet::from([envelope.revision_id]));
        let (_, fresh) =
            self.publish_observed(operation, fingerprint.clone(), envelope, vec![], binding)?;
        Ok(fresh.then(|| WriterFlowGrant {
            operation,
            fingerprint,
            conversation,
            correlation,
            effect,
            draft,
            store: self.store.as_ref() as *const Store as usize,
            root: current,
        }))
    }
    pub(crate) fn select_writer_for_flow(
        &self,
        conversation: Uuid,
        draft: Uuid,
        expected: ExpectedHeads,
    ) -> Result<WriterSelection> {
        self.complete_store()?;
        let identity = conversation.to_string();
        let snapshot = self.store.snapshot_heads_matching(
            &[
                Namespace::Conversation,
                Namespace::Source,
                Namespace::ExportAssociation,
            ],
            MAX_ITEMS,
            |e| {
                let payload = &e.payload["record"];
                payload["conversation"].as_str() == Some(&identity)
                    || payload["acknowledgment"]["conversation"].as_str() == Some(&identity)
                    || (e.namespace == Namespace::Conversation && e.record_id == conversation)
            },
        )?;
        let records = Self::inspect_snapshot(conversation, false, &snapshot)?;
        self.validate_evidence_records(&records)?;
        let roots: BTreeSet<_> = snapshot
            .iter()
            .filter(|e| e.namespace == Namespace::Conversation && e.record_id == conversation)
            .map(|e| e.revision_id)
            .collect();
        ensure!(roots == expected.0, "selected root changed");
        let drafts: Vec<_> = snapshot
            .iter()
            .filter(|e| e.namespace == Namespace::Conversation && e.record_id == draft)
            .collect();
        ensure!(drafts.len() == 1, "draft absent or conflicted");
        let Record::Turn(turn) = Self::decode(drafts[0])? else {
            bail!("not a turn")
        };
        ensure!(turn.sources.len() == 1, "ambiguous Writer source");
        let sources: Vec<_> = snapshot
            .iter()
            .filter(|e| e.namespace == Namespace::Source && e.record_id == turn.sources[0])
            .collect();
        ensure!(sources.len() == 1, "source absent or conflicted");
        let selected = WriterSelection {
            root: expected,
            draft: drafts[0].clone(),
            source: sources[0].clone(),
        };
        let (turn, _) = selected.values()?;
        ensure!(
            turn.conversation == conversation,
            "foreign Writer selection"
        );
        Ok(selected)
    }
}
