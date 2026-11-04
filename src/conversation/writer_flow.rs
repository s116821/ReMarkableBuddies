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
    notebook: Option<NotebookSelection>,
}
struct NotebookSelection {
    plan: Envelope,
    guards: Vec<Envelope>,
    live: Arc<Store>,
    // Joined generation retains its consumed fresh witness. Explicit export of
    // a saved draft instead requires the existing fresh outer intent path.
    _fresh: Option<Box<notebook::NotebookFinalized>>,
}
impl WriterSelection {
    pub(crate) fn published(root: &Envelope, draft: &Envelope, source: &Envelope) -> Result<Self> {
        let value = Self {
            root: ExpectedHeads(BTreeSet::from([root.revision_id])),
            draft: draft.clone(),
            source: source.clone(),
            notebook: None,
        };
        value.values()?;
        Ok(value)
    }
    pub(crate) fn values(&self) -> Result<(Turn, NormalizedTextSource)> {
        ensure!(
            self.notebook.is_none(),
            "notebook selection requires its typed export path"
        );
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
    pub(crate) fn published_notebook(
        ledger: &Ledger,
        finalized: notebook::NotebookFinalized,
    ) -> Result<Self> {
        ensure!(
            finalized.is_for_store(ledger.store()),
            "foreign notebook final witness"
        );
        let selected = Self {
            root: ExpectedHeads(BTreeSet::from([finalized.root().revision_id])),
            draft: finalized.draft().clone(),
            source: finalized.source().clone(),
            notebook: Some(NotebookSelection {
                plan: finalized.plan().clone(),
                guards: finalized.guards().to_vec(),
                live: ledger.store().clone(),
                _fresh: Some(Box::new(finalized)),
            }),
        };
        selected.notebook_values()?;
        Ok(selected)
    }
    fn notebook_values(&self) -> Result<(Turn, ArchivedNormalizedSource, notebook::NotebookPlan)> {
        let selected = self
            .notebook
            .as_ref()
            .context("notebook selection absent")?;
        let Record::Turn(draft) = Ledger::decode(&self.draft)? else {
            bail!("notebook draft absent")
        };
        let Record::ArchivedNormalizedSource(source) = Ledger::decode(&self.source)? else {
            bail!("notebook archive absent")
        };
        let Record::NotebookPlan(plan) = Ledger::decode(&selected.plan)? else {
            bail!("notebook plan absent")
        };
        ensure!(
            draft.mode == Mode::Writer
                && draft.role == Role::Assistant
                && draft.outcome == Outcome::Generated
                && draft.conversation == source.conversation
                && draft.sources == vec![source.id]
                && draft.exchange == plan.operation
                && plan.conversation == draft.conversation
                && plan.archive_id == source.id,
            "invalid notebook final selection"
        );
        let bytes = source.load(&selected.live)?;
        plan.validate_source(&source, &bytes)?;
        plan.export_revision(
            draft.id,
            draft
                .text
                .as_deref()
                .context("notebook draft text absent")?,
        )?;
        crate::writer::parse_draft(
            draft
                .text
                .as_deref()
                .context("notebook draft text absent")?,
        )?;
        Ok((draft, *source, *plan))
    }
    fn draft_value(&self) -> Result<Turn> {
        if self.notebook.is_some() {
            Ok(self.notebook_values()?.0)
        } else {
            Ok(self.values()?.0)
        }
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
        if self.selection.notebook.is_some() {
            let (draft, source, plan) = self.selection.notebook_values()?;
            let text = draft
                .text
                .as_deref()
                .context("notebook draft text absent")?;
            let (title, markdown) = crate::writer::parse_draft(text)?;
            let note = crate::note_export::PreparedNote {
                conversation: draft.conversation,
                correlation: self.grant.correlation,
                effect_operation: self.grant.effect,
                source_scope: format!(
                    "normalized-document:{}:{}",
                    source.document,
                    match source.selection {
                        TextScope::Page { page } => format!("page:{page}"),
                        TextScope::SuppliedNotebook => "supplied-notebook".into(),
                    }
                ),
                source_revision: plan.export_revision(draft.id, text)?,
                title: title.into(),
                markdown: markdown.into(),
            };
            note.render(crate::note_export::CorrelationLocation::Title)?;
            return Ok(note);
        }
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
        let draft = self.selection.draft_value()?;
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
        if let Some(notebook) = &self.selection.notebook {
            ensure!(
                Arc::ptr_eq(&notebook.live, ledger.store()),
                "foreign notebook selection store"
            );
            return Ledger::fingerprint(&(
                "writer-notebook-export-attempt-v1",
                association,
                self.grant.operation,
                &self.selection.root.0,
                self.selection.draft.revision_id,
                self.selection.source.revision_id,
                notebook.plan.revision_id,
            ));
        }
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
        if let Some(notebook) = &self.selection.notebook {
            self.selection.notebook_values()?;
            ensure!(
                Arc::ptr_eq(&notebook.live, ledger.store()),
                "foreign notebook selection store"
            );
            let mut records = notebook.guards.clone();
            let outer = ledger
                .store
                .value(
                    Namespace::Conversation,
                    Ledger::receipt_id(self.grant.operation),
                )?
                .context("notebook outer intent receipt absent")?;
            ensure!(
                matches!(Ledger::decode(&outer)?,Record::Receipt(ref receipt)
                if receipt.fingerprint == self.grant.fingerprint && receipt.acknowledgment.operation == self.grant.operation
                && self.grant.root.0.contains(&receipt.acknowledgment.applied_root_revision)),
                "notebook outer intent changed"
            );
            records.push(outer);
            records.push(self.selection.draft.clone());
            ensure!(
                records
                    .iter()
                    .map(|e| (e.namespace, e.record_id))
                    .collect::<BTreeSet<_>>()
                    .len()
                    == records.len(),
                "duplicate notebook export guard"
            );
            return Ok(records
                .iter()
                .map(|old| {
                    let mut guard = old.clone();
                    guard.parents = BTreeSet::from([old.revision_id]);
                    guard.revision_id = Uuid::new_v4();
                    guard.operation_id = Uuid::new_v4();
                    guard.actor_id = ledger.store.actor_id;
                    guard
                })
                .collect());
        }
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

pub(crate) struct WriterExportGuard {
    heads: Vec<(Namespace, Uuid, Uuid)>,
    archive: Option<Box<ArchivedNormalizedSource>>,
    live: Arc<Store>,
}
impl WriterExportGuard {
    pub(crate) fn published(
        root: &Envelope,
        records: &[Envelope],
        live: Arc<Store>,
    ) -> Result<Self> {
        let mut archive = None;
        for record in records {
            if record.namespace == Namespace::Source
                && record.domain_schema_version == notebook::NOTEBOOK_SOURCE_SCHEMA
            {
                let Record::ArchivedNormalizedSource(source) = Ledger::decode(record)? else {
                    bail!("notebook export archive absent")
                };
                ensure!(archive.is_none(), "ambiguous notebook export archive");
                archive = Some(source);
            }
        }
        Ok(Self {
            heads: std::iter::once(root)
                .chain(records)
                .map(|e| (e.namespace, e.record_id, e.revision_id))
                .collect(),
            archive,
            live,
        })
    }
    pub(crate) fn current(&self, ledger: &Ledger) -> Result<bool> {
        ledger.complete_store()?;
        if !Arc::ptr_eq(&self.live, ledger.store()) {
            return Ok(false);
        }
        if self
            .archive
            .as_ref()
            .is_some_and(|archive| archive.load(ledger.store()).is_err())
        {
            return Ok(false);
        }
        let heads = ledger.store.snapshot_heads_matching(
            &[
                Namespace::Conversation,
                Namespace::Source,
                Namespace::ExportAssociation,
            ],
            MAX_ITEMS,
            |e| {
                self.heads
                    .iter()
                    .any(|(namespace, id, _)| e.namespace == *namespace && e.record_id == *id)
            },
        )?;
        Ok(self.heads.iter().all(|(namespace, id, revision)| {
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
        let mut selected = WriterSelection {
            root: expected,
            draft: drafts[0].clone(),
            source: sources[0].clone(),
            notebook: None,
        };
        if matches!(
            Self::decode(sources[0])?,
            Record::ArchivedNormalizedSource(_)
        ) {
            let plan_id = notebook::NotebookPlan::identity(turn.exchange);
            let plan = snapshot
                .iter()
                .find(|e| e.namespace == Namespace::Conversation && e.record_id == plan_id)
                .context("saved notebook plan absent")?
                .clone();
            let Record::NotebookPlan(value) = Self::decode(&plan)? else {
                bail!("saved notebook plan absent")
            };
            let mut stages = Vec::new();
            for envelope in &snapshot {
                if envelope.kind != Kind::Value
                    || envelope.domain_schema_version != notebook::NOTEBOOK_PROCESSING_SCHEMA
                    || envelope.namespace != Namespace::Conversation
                {
                    continue;
                }
                if let Record::NotebookStage(stage) = Self::decode(envelope)? {
                    if stage.plan_id == plan_id {
                        ensure!(
                            stage.outcome == Some(notebook::NotebookStageOutcome::Generated),
                            "saved notebook stage incomplete"
                        );
                        stages.push((envelope.clone(), stage));
                    }
                }
            }
            ensure!(
                stages.len() == value.chunks.len() + 1,
                "saved notebook stages incomplete"
            );
            let (_, reduce) = stages
                .iter()
                .find(|(_, stage)| stage.kind == notebook::NotebookStageKind::Reduce)
                .context("saved notebook reducer absent")?;
            ensure!(
                reduce.output == turn.text,
                "saved notebook final text mismatch"
            );
            for (id, _, digest) in &reduce.predecessors {
                ensure!(
                    stages.iter().any(|(_, stage)| stage.id == *id
                        && stage.kind == notebook::NotebookStageKind::Map
                        && stage.observed_output_digest.as_ref() == Some(digest)),
                    "saved notebook map result changed"
                );
            }
            let final_operation = Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("urn:rmb:writer-notebook-transition:v1:{plan_id}:finalize").as_bytes(),
            );
            let receipt = snapshot
                .iter()
                .find(|e| {
                    e.namespace == Namespace::Conversation
                        && e.record_id == Self::receipt_id(final_operation)
                })
                .context("saved notebook final receipt absent")?;
            let Record::Receipt(receipt) = Self::decode(receipt)? else {
                bail!("saved notebook final receipt absent")
            };
            ensure!(
                receipt.acknowledgment.operation == final_operation
                    && receipt.acknowledgment.conversation == conversation
                    && receipt.acknowledgment.records.contains(&draft),
                "saved notebook final receipt mismatch"
            );
            let candidates = self.store.snapshot_revisions_matching(
                &[Namespace::Conversation],
                MAX_ITEMS,
                |e| e.record_id == reduce.id,
            )?;
            let mut observed = false;
            for candidate in candidates {
                let Record::NotebookStage(result) = Self::decode(&candidate)? else {
                    bail!("saved notebook reducer shape changed")
                };
                if result.outcome != Some(notebook::NotebookStageOutcome::Generated)
                    || result.finish_fingerprint()? != reduce.finish_fingerprint()?
                {
                    continue;
                }
                let reference = self.store.notebook_revision_reference(&candidate)?;
                if receipt.fingerprint
                    == Self::fingerprint(&(
                        "finalize-writer-notebook-v1",
                        plan_id,
                        &value.core_digest,
                        reference.sha256,
                        turn.text
                            .as_deref()
                            .context("saved notebook draft text absent")?,
                    ))?
                {
                    observed = true;
                    break;
                }
            }
            ensure!(observed, "saved notebook final publication unverified");
            let mut guards = vec![selected.source.clone(), plan.clone()];
            guards.extend(stages.into_iter().map(|(envelope, _)| envelope));
            selected.notebook = Some(NotebookSelection {
                plan,
                guards,
                live: self.store.clone(),
                _fresh: None,
            });
        }
        let turn = selected.draft_value()?;
        ensure!(
            turn.conversation == conversation,
            "foreign Writer selection"
        );
        Ok(selected)
    }
}
