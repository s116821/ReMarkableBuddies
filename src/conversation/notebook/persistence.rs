//! Consumed invocation cursors for actual Store publications, never reopen grants.
use super::*;

#[derive(Clone)]
pub struct NotebookAdmission {
    pub plan: NotebookPlan,
    pub archive: ArchivedNormalizedSource,
    pub canonical: Vec<u8>,
    pub user: Turn,
    pub expected: ExpectedHeads,
    /// Exact previously selected eligible history/prior draft envelopes.
    pub guards: Vec<Envelope>,
}
pub struct NotebookRunCursor {
    plan: NotebookPlan,
    root: ExpectedHeads,
    guards: Vec<Envelope>,
    plan_revision: String,
    results: Vec<Envelope>,
}
pub struct NotebookPreparedStage {
    cursor: NotebookRunCursor,
    prepared: Envelope,
}
impl NotebookPreparedStage {
    pub fn input_digest(&self) -> Result<String> {
        let Record::NotebookStage(stage) = Ledger::decode(&self.prepared)? else {
            bail!("invalid prepared notebook stage")
        };
        Ok(stage.input_digest)
    }
}
pub struct NotebookResult {
    pub outcome: NotebookStageOutcome,
    pub invoked: bool,
    pub output: Option<String>,
    pub observed_output_digest: Option<String>,
    pub observed_output_bytes: Option<u64>,
    pub failure: Option<NotebookFailure>,
}
fn encoded_revision(envelope: &Envelope) -> Result<String> {
    Ok(digest(&serde_json::to_vec(envelope)?))
}
fn guarded(envelope: &Envelope) -> Envelope {
    let mut next = envelope.clone();
    next.parents = BTreeSet::from([envelope.revision_id]);
    next.revision_id = Uuid::new_v4();
    next.operation_id = Uuid::new_v4();
    next
}

impl Ledger {
    /// A progress check reads one actual Store snapshot and grants no new cursor.
    pub fn notebook_stage_current(&self, prepared: &NotebookPreparedStage) -> Result<bool> {
        self.complete_store()?;
        let cursor = &prepared.cursor;
        let current = self.store.snapshot_heads_matching(
            &[Namespace::Conversation, Namespace::Source],
            MAX_ITEMS,
            |e| {
                (e.namespace == Namespace::Conversation && e.record_id == cursor.plan.conversation)
                    || cursor.guards.iter().any(|guard| {
                        guard.namespace == e.namespace && guard.record_id == e.record_id
                    })
            },
        )?;
        let revisions = |namespace, id| {
            current
                .iter()
                .filter(|e| e.namespace == namespace && e.record_id == id)
                .map(|e| e.revision_id)
                .collect::<BTreeSet<_>>()
        };
        Ok(
            revisions(Namespace::Conversation, cursor.plan.conversation) == cursor.root.0
                && cursor.guards.iter().all(|guard| {
                    revisions(guard.namespace, guard.record_id)
                        == BTreeSet::from([guard.revision_id])
                }),
        )
    }
    /// Only the final reducer result becomes ordinary visible chronology.
    /// The returned historical facts are not a destination execution grant.
    pub fn finalize_notebook(
        &self,
        cursor: NotebookRunCursor,
    ) -> Result<Option<(WriteResult, Turn)>> {
        let result = cursor.results.last().context("notebook reducer absent")?;
        let Record::NotebookStage(reducer) = Self::decode(result)? else {
            bail!("notebook reducer absent")
        };
        ensure!(
            cursor.results.len() == cursor.plan.chunks.len() + 1
                && reducer.kind == NotebookStageKind::Reduce
                && reducer.outcome == Some(NotebookStageOutcome::Generated),
            "notebook plan incomplete"
        );
        let text = reducer.output.context("notebook final text absent")?;
        crate::writer::parse_draft(&text)?;
        let operation = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "urn:rmb:writer-notebook-transition:v1:{}:finalize",
                cursor.plan.id
            )
            .as_bytes(),
        );
        let fingerprint = Self::fingerprint(&(
            "finalize-writer-notebook-v1",
            cursor.plan.id,
            &cursor.plan.core_digest,
            encoded_revision(result)?,
            &text,
        ))?;
        if self.replay(operation, &fingerprint)?.is_some() {
            return Ok(None);
        }
        let (_, mut root) = self.live_root(cursor.plan.conversation, Some(&cursor.root))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let turn = Turn {
            id: Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("urn:rmb:writer:v1:{}:draft", cursor.plan.operation).as_bytes(),
            ),
            conversation: cursor.plan.conversation,
            exchange: cursor.plan.operation,
            sequence: root.next_sequence,
            role: Role::Assistant,
            mode: Mode::Writer,
            outcome: Outcome::Generated,
            text: Some(text),
            sources: vec![cursor.plan.archive_id],
            correction_of: None,
            created_ms: now,
            updated_ms: now,
            completion: None,
        };
        Self::validate_turn(&turn)?;
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("notebook chronology exhausted")?;
        root.updated_ms = now;
        let binding = root.binding;
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            cursor.root,
            Record::Root(root),
            vec![],
        )?;
        let mut records = cursor.guards.iter().map(guarded).collect::<Vec<_>>();
        records.push(self.envelope(
            Namespace::Conversation,
            turn.id,
            ExpectedHeads::default(),
            Record::Turn(turn.clone()),
            vec![],
        )?);
        let (result, fresh) =
            self.publish_observed(operation, fingerprint, root, records, binding)?;
        Ok(fresh.then_some((result, turn)))
    }
    /// Atomic user/source/plan admission through the existing Ledger receipt path.
    /// Historical replay returns no cursor, including a committed lost acknowledgment.
    pub fn admit_notebook(&self, input: NotebookAdmission) -> Result<Option<NotebookRunCursor>> {
        let NotebookAdmission {
            mut plan,
            archive,
            canonical,
            mut user,
            expected,
            guards,
        } = input;
        let source = archive.validate_bytes(&canonical)?;
        plan.validate_source(&archive, &source)?;
        let calls = plan.chunks.len() + 1;
        let history = guards.len();
        ensure!(
            history <= 64
                && 11 + 2 * history + 12 * calls + 2 * history * calls + calls * calls <= 256
                && 7 + history + 3 * calls <= 64,
            "notebook publication reservation exceeds bound"
        );
        ensure!(
            guards
                .iter()
                .map(|e| (e.namespace, e.record_id))
                .collect::<BTreeSet<_>>()
                .len()
                == history,
            "duplicate notebook history guard"
        );
        Self::validate_turn(&user)?;
        ensure!(
            user.id
                == Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!("urn:rmb:writer:v1:{}:user", plan.operation).as_bytes()
                )
                && user.conversation == plan.conversation
                && user.role == Role::User
                && user.mode == Mode::Writer
                && user.outcome == Outcome::Interpreted
                && user.text.as_ref() == Some(&plan.request)
                && user.sequence == 0
                && user.sources == vec![archive.id],
            "invalid notebook user admission"
        );
        // Source revision is generated below and deliberately excluded from core.
        // Replay fingerprints use stable logical input, never a random envelope.
        let fingerprint = Self::fingerprint(&(
            "admit-writer-notebook-v1",
            &plan.core_digest,
            &plan.map_input_digests,
            &plan.reducer_prefix_digest,
            plan.reducer_reserved_bytes,
            &archive,
            &user,
            guards
                .iter()
                .map(|e| (e.namespace, e.record_id, e.revision_id))
                .collect::<Vec<_>>(),
        ))?;
        if self.replay(plan.operation, &fingerprint)?.is_some() {
            return Ok(None);
        }
        let (_, mut root) = self.live_root(plan.conversation, Some(&expected))?;
        ensure!(
            (archive.turn == user.id) == plan.prior.is_none()
                && user.correction_of == plan.prior.as_ref().map(|prior| prior.0),
            "invalid notebook original/refinement linkage"
        );
        if let Some((id, revision, text)) = &plan.prior {
            let selected = guards
                .iter()
                .find(|e| e.record_id == *id && e.namespace == Namespace::Conversation)
                .context("notebook prior draft guard absent")?;
            ensure!(
                encoded_revision(selected)? == *revision
                    && matches!(Self::decode(selected)?,Record::Turn(ref draft)
                    if draft.role == Role::Assistant && draft.mode == Mode::Writer
                    && draft.outcome == Outcome::Generated && draft.sources == vec![archive.id]
                    && draft.text.as_ref().is_some_and(|body| digest(body.as_bytes()) == *text)),
                "notebook prior draft mismatch"
            );
        }
        let original = if archive.turn == user.id {
            self.envelope(
                Namespace::Source,
                archive.id,
                ExpectedHeads::default(),
                Record::ArchivedNormalizedSource(Box::new(archive.clone())),
                vec![archive.media.clone()],
            )?
        } else {
            let prior = self
                .store
                .value(Namespace::Source, archive.id)?
                .context("notebook source absent")?;
            ensure!(
                matches!(Self::decode(&prior)?,Record::ArchivedNormalizedSource(ref old) if **old == archive),
                "notebook source changed"
            );
            prior
        };
        plan.source_revision = encoded_revision(&original)?;
        let plan_envelope = self.envelope(
            Namespace::Conversation,
            plan.id,
            ExpectedHeads::default(),
            Record::NotebookPlan(Box::new(plan.clone())),
            vec![],
        )?;
        let plan_revision = encoded_revision(&plan_envelope)?;
        let mut records = Vec::new();
        for guard in guards {
            ensure!(
                self.store.value(guard.namespace, guard.record_id)?.as_ref() == Some(&guard),
                "notebook history changed"
            );
            ensure!(
                guard.namespace == Namespace::Conversation
                    && matches!(Self::decode(&guard)?,Record::Turn(ref t) if t.conversation == plan.conversation),
                "invalid notebook history guard"
            );
            records.push(guarded(&guard));
        }
        records.push(if archive.turn == user.id {
            original
        } else {
            guarded(&original)
        });
        records.push(plan_envelope);
        user.sequence = root.next_sequence;
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("notebook chronology exhausted")?;
        root.updated_ms = user.updated_ms;
        let user_envelope = self.envelope(
            Namespace::Conversation,
            user.id,
            ExpectedHeads::default(),
            Record::Turn(user),
            vec![],
        )?;
        records.push(user_envelope);
        let cursor_guards = records.clone();
        let binding = root.binding;
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            expected,
            Record::Root(root),
            vec![],
        )?;
        let root_heads = ExpectedHeads(BTreeSet::from([root.revision_id]));
        // Verified staging remains unreachable if the receipt publication fails.
        self.store.stage_blob(
            &ObjectRef {
                sha256: archive.media.sha256,
                bytes: archive.media.bytes,
            },
            std::io::Cursor::new(canonical),
        )?;
        let (_, fresh) =
            self.publish_observed(plan.operation, fingerprint, root, records, binding)?;
        Ok(fresh.then_some(NotebookRunCursor {
            plan,
            root: root_heads,
            guards: cursor_guards,
            plan_revision,
            results: vec![],
        }))
    }
    /// Consumes the last positively observed cursor. No refresh/reopen operation
    /// exists: a conflict, prior receipt or unknown acknowledgment ends this run.
    pub fn prepare_notebook_stage(
        &self,
        cursor: NotebookRunCursor,
        kind: NotebookStageKind,
        ordinal: usize,
        input_digest: String,
    ) -> Result<Option<NotebookPreparedStage>> {
        let plan = &cursor.plan;
        let mut predecessors = Vec::new();
        match kind {
            NotebookStageKind::Map => ensure!(
                ordinal == cursor.results.len()
                    && ordinal < plan.chunks.len()
                    && plan.map_input_digests[ordinal] == input_digest,
                "invalid notebook map progression"
            ),
            NotebookStageKind::Reduce => {
                ensure!(
                    ordinal == 0 && cursor.results.len() == plan.chunks.len(),
                    "incomplete notebook reduction"
                );
                for envelope in &cursor.results {
                    let Record::NotebookStage(result) = Self::decode(envelope)? else {
                        bail!("notebook map result absent")
                    };
                    ensure!(
                        result.kind == NotebookStageKind::Map
                            && result.outcome == Some(NotebookStageOutcome::Generated),
                        "notebook map result unavailable"
                    );
                    predecessors.push((
                        result.id,
                        encoded_revision(envelope)?,
                        result
                            .observed_output_digest
                            .context("notebook output digest absent")?,
                    ));
                }
            }
        }
        let stage = NotebookStage {
            id: NotebookStage::identity(plan.operation, kind, ordinal),
            conversation: plan.conversation,
            operation: plan.operation,
            plan_id: plan.id,
            core_digest: plan.core_digest.clone(),
            plan_revision: cursor.plan_revision.clone(),
            kind,
            ordinal,
            archive_id: plan.archive_id,
            source_revision: plan.source_revision.clone(),
            input_digest,
            predecessors,
            prepared_revision: None,
            outcome: None,
            invoked: None,
            output: None,
            observed_output_digest: None,
            observed_output_bytes: None,
            failure: None,
        };
        stage.validate()?;
        let operation = stage.transition(false);
        let fingerprint = stage.prepare_fingerprint()?;
        if self.replay(operation, &fingerprint)?.is_some() {
            return Ok(None);
        }
        let envelope = self.envelope(
            Namespace::Conversation,
            stage.id,
            ExpectedHeads::default(),
            Record::NotebookStage(Box::new(stage)),
            vec![],
        )?;
        let Some(cursor) =
            self.publish_notebook_cursor(cursor, operation, fingerprint, envelope.clone())?
        else {
            return Ok(None);
        };
        Ok(Some(NotebookPreparedStage {
            cursor,
            prepared: envelope,
        }))
    }
    pub fn finish_notebook_stage(
        &self,
        prepared: NotebookPreparedStage,
        result: NotebookResult,
    ) -> Result<Option<NotebookRunCursor>> {
        let NotebookPreparedStage {
            mut cursor,
            prepared,
        } = prepared;
        let Record::NotebookStage(mut stage) = Self::decode(&prepared)? else {
            bail!("notebook preparation absent")
        };
        stage.prepared_revision = Some(encoded_revision(&prepared)?);
        stage.outcome = Some(result.outcome);
        stage.invoked = Some(result.invoked);
        stage.output = result.output;
        stage.observed_output_digest = result.observed_output_digest;
        stage.observed_output_bytes = result.observed_output_bytes;
        stage.failure = result.failure;
        stage.validate()?;
        let operation = stage.transition(true);
        let fingerprint = stage.finish_fingerprint()?;
        if self.replay(operation, &fingerprint)?.is_some() {
            return Ok(None);
        }
        let generated = stage.outcome == Some(NotebookStageOutcome::Generated);
        let envelope = self.envelope(
            Namespace::Conversation,
            stage.id,
            ExpectedHeads(BTreeSet::from([prepared.revision_id])),
            Record::NotebookStage(stage),
            vec![],
        )?;
        // Preparation was already guarded by its own exact expected parent.
        cursor.guards.retain(|e| e.record_id != prepared.record_id);
        let Some(mut cursor) =
            self.publish_notebook_cursor(cursor, operation, fingerprint, envelope.clone())?
        else {
            return Ok(None);
        };
        if !generated {
            return Ok(None);
        }
        cursor.results.push(envelope);
        Ok(Some(cursor))
    }
    fn publish_notebook_cursor(
        &self,
        mut cursor: NotebookRunCursor,
        operation: Uuid,
        fingerprint: String,
        record: Envelope,
    ) -> Result<Option<NotebookRunCursor>> {
        let (_, root) = self.live_root(cursor.plan.conversation, Some(&cursor.root))?;
        let binding = root.binding;
        let root = self.envelope(
            Namespace::Conversation,
            root.id,
            cursor.root,
            Record::Root(root),
            vec![],
        )?;
        let next = ExpectedHeads(BTreeSet::from([root.revision_id]));
        let mut records = cursor.guards.iter().map(guarded).collect::<Vec<_>>();
        records.push(record);
        let guards = records.clone();
        let (_, fresh) = self.publish_observed(operation, fingerprint, root, records, binding)?;
        if !fresh {
            return Ok(None);
        }
        cursor.root = next;
        cursor.guards = guards;
        Ok(Some(cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Fault, StorePaths};
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("notebook-stages-{}", Uuid::new_v4())))
        }
        fn paths(&self) -> StorePaths {
            StorePaths {
                data: self.0.join("data"),
                cache: self.0.join("cache"),
                credentials: self.0.join("credentials"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn admission(ledger: &Ledger) -> NotebookAdmission {
        let operation = Uuid::new_v4();
        let conversation = Uuid::new_v4();
        ledger.create(conversation, Uuid::new_v4(), 0).unwrap();
        let user_id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:writer:v1:{operation}:user").as_bytes(),
        );
        let document = crate::retrieval::NormalizedDocument {
            document: Uuid::new_v4(),
            format: SourceFormat::Notebook,
            source_sha256: "a".repeat(64),
            extractor_revision: "stage-fixture-v1".into(),
            units: vec![crate::retrieval::SourceUnit {
                page: Some(1),
                section: None,
                text: "Force = 10 N.".into(),
            }],
        };
        let raw = serde_json::to_vec(&document).unwrap();
        let source = ValidatedNotebook::from_bytes(&raw, &digest(&raw)).unwrap();
        let archive = ArchivedNormalizedSource::from_notebook(
            conversation,
            user_id,
            &source,
            TextScope::SuppliedNotebook,
            None,
        )
        .unwrap();
        let mut plan = NotebookPlan {
            id: NotebookPlan::identity(operation),
            conversation,
            operation,
            archive_id: archive.id,
            source_revision: "b".repeat(64),
            source_identity_digest: archive.identity_digest().unwrap(),
            request: "Make notes".into(),
            guidance: None,
            personalization_digest: digest(b"effective personalization"),
            history_digest: digest(b"empty history"),
            prior: None,
            caller_max_model_calls: 2,
            chunks: source
                .coverage(&TextScope::SuppliedNotebook, 2)
                .unwrap()
                .chunks()
                .to_vec(),
            core_digest: String::new(),
            map_input_digests: vec![digest(b"map payload")],
            reducer_prefix_digest: digest(b"reducer prefix"),
            reducer_reserved_bytes: 8192,
        };
        plan.core_digest = plan.core_identity().unwrap();
        let user = Turn {
            id: user_id,
            conversation,
            exchange: operation,
            sequence: 0,
            role: Role::User,
            mode: Mode::Writer,
            outcome: Outcome::Interpreted,
            text: Some(plan.request.clone()),
            sources: vec![archive.id],
            correction_of: None,
            created_ms: 0,
            updated_ms: 0,
            completion: None,
        };
        NotebookAdmission {
            plan,
            archive,
            canonical: source.canonical_bytes().to_vec(),
            user,
            expected: ledger
                .expected(Namespace::Conversation, conversation)
                .unwrap(),
            guards: vec![],
        }
    }
    fn generated(text: &str) -> NotebookResult {
        NotebookResult {
            outcome: NotebookStageOutcome::Generated,
            invoked: true,
            output: Some(text.into()),
            observed_output_digest: Some(digest(text.as_bytes())),
            observed_output_bytes: Some(text.len() as u64),
            failure: None,
        }
    }
    #[test]
    fn real_store_map_reduce_final_preserves_two_visible_turns_and_replay_has_no_cursor() {
        let f = Fixture::new();
        let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
        let ledger = Ledger::new(store.clone());
        let input = admission(&ledger);
        let replay = input.clone();
        let conversation = input.plan.conversation;
        let map_digest = input.plan.map_input_digests[0].clone();
        let cursor = ledger.admit_notebook(input).unwrap().unwrap();
        assert!(ledger.admit_notebook(replay.clone()).unwrap().is_none());
        let prepared = ledger
            .prepare_notebook_stage(cursor, NotebookStageKind::Map, 0, map_digest.clone())
            .unwrap()
            .unwrap();
        assert_eq!(prepared.input_digest().unwrap(), map_digest);
        let cursor = ledger
            .finish_notebook_stage(prepared, generated("Force = 10 N."))
            .unwrap()
            .unwrap();
        let prepared = ledger
            .prepare_notebook_stage(
                cursor,
                NotebookStageKind::Reduce,
                0,
                digest(b"actual reducer payload"),
            )
            .unwrap()
            .unwrap();
        let cursor = ledger
            .finish_notebook_stage(prepared, generated("# Force\n\nForce = 10 N."))
            .unwrap()
            .unwrap();
        let (_, draft) = ledger.finalize_notebook(cursor).unwrap().unwrap();
        assert_eq!(draft.sequence, 1);
        let records = ledger.inspect(conversation, false).unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r, Record::Turn(_)))
                .count(),
            2
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| matches!(r, Record::NotebookStage(_)))
                .count(),
            2
        );
        drop(ledger);
        drop(store);
        let ledger = Ledger::new(Arc::new(Store::open(f.paths()).unwrap()));
        assert!(ledger.admit_notebook(replay).unwrap().is_none());
    }
    #[test]
    fn lost_prepare_or_finish_ack_retains_facts_without_automatic_continuation() {
        for finish in [false, true] {
            let f = Fixture::new();
            let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
            let ledger = Ledger::new(store.clone());
            let input = admission(&ledger);
            let replay = input.clone();
            let conversation = input.plan.conversation;
            let map_digest = input.plan.map_input_digests[0].clone();
            let cursor = ledger.admit_notebook(input).unwrap().unwrap();
            if !finish {
                store.set_fault(Fault::AfterCommit).unwrap();
            }
            let prepared = ledger
                .prepare_notebook_stage(cursor, NotebookStageKind::Map, 0, map_digest)
                .unwrap();
            if finish {
                store.set_fault(Fault::AfterCommit).unwrap();
                assert!(ledger
                    .finish_notebook_stage(prepared.unwrap(), generated("map output"))
                    .unwrap()
                    .is_none());
            } else {
                assert!(prepared.is_none());
            }
            assert!(ledger.admit_notebook(replay).unwrap().is_none());
            assert_eq!(
                ledger
                    .inspect(conversation, false)
                    .unwrap()
                    .iter()
                    .filter(|r| matches!(r, Record::Turn(_)))
                    .count(),
                1
            );
        }
    }
    #[test]
    fn intervening_root_revision_consumes_stale_cursor_and_prevents_stage_preparation() {
        let f = Fixture::new();
        let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
        let ledger = Ledger::new(store.clone());
        let input = admission(&ledger);
        let conversation = input.plan.conversation;
        let map = input.plan.map_input_digests[0].clone();
        let cursor = ledger.admit_notebook(input).unwrap().unwrap();
        let root = store
            .value(Namespace::Conversation, conversation)
            .unwrap()
            .unwrap();
        store.commit(vec![guarded(&root)], BTreeMap::new()).unwrap();
        assert!(ledger
            .prepare_notebook_stage(cursor, NotebookStageKind::Map, 0, map)
            .is_err());
        assert!(!ledger
            .inspect(conversation, false)
            .unwrap()
            .iter()
            .any(|r| matches!(r, Record::NotebookStage(_))));
    }
    #[test]
    fn direct_store_cannot_publish_omitted_coverage_or_result_with_absent_preparation() {
        let f = Fixture::new();
        let store = Arc::new(Store::prepare_notebook(f.paths()).unwrap());
        let ledger = Ledger::new(store.clone());
        let input = admission(&ledger);
        let plan_id = input.plan.id;
        let operation = input.plan.operation;
        let map = input.plan.map_input_digests[0].clone();
        let cursor = ledger.admit_notebook(input).unwrap().unwrap();
        let original = store
            .value(Namespace::Conversation, plan_id)
            .unwrap()
            .unwrap();
        let Record::NotebookPlan(mut plan) = Ledger::decode(&original).unwrap() else {
            panic!("plan absent")
        };
        plan.chunks=vec![serde_json::from_value(serde_json::json!({"ordinal":0,"authoritative":[[0,0,5]],"before_context":[],"after_context":[]})).unwrap()];
        plan.core_digest = plan.core_identity().unwrap();
        plan.validate().unwrap();
        let mut wrong = guarded(&original);
        wrong.payload = serde_json::to_value(Record::NotebookPlan(plan)).unwrap();
        assert!(store.commit(vec![wrong], BTreeMap::new()).is_err());
        assert_eq!(
            store
                .value(Namespace::Conversation, plan_id)
                .unwrap()
                .unwrap(),
            original
        );
        let prepared = ledger
            .prepare_notebook_stage(cursor, NotebookStageKind::Map, 0, map)
            .unwrap()
            .unwrap();
        assert!(ledger.notebook_stage_current(&prepared).unwrap());
        let id = NotebookStage::identity(operation, NotebookStageKind::Map, 0);
        let original = store.value(Namespace::Conversation, id).unwrap().unwrap();
        let Record::NotebookStage(mut stage) = Ledger::decode(&original).unwrap() else {
            panic!("stage absent")
        };
        stage.prepared_revision = Some("f".repeat(64));
        stage.outcome = Some(NotebookStageOutcome::Generated);
        stage.invoked = Some(true);
        stage.output = Some("output".into());
        stage.observed_output_digest = Some(digest(b"output"));
        stage.observed_output_bytes = Some(6);
        stage.validate().unwrap();
        let mut wrong = guarded(&original);
        wrong.payload = serde_json::to_value(Record::NotebookStage(stage)).unwrap();
        assert!(store.commit(vec![wrong], BTreeMap::new()).is_err());
        assert!(ledger.notebook_stage_current(&prepared).unwrap());
    }
}
