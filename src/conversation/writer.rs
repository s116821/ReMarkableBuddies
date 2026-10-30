//! Explicit supplied text is historical source data, never a native observation.
use super::*;
use crate::retrieval::{Index, NormalizedDocument};
use serde::{Deserialize, Serialize};

pub const MAX_WRITER_SOURCE: usize = 48 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "scope", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TextScope {
    Page { page: u32 },
    SuppliedNotebook,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NormalizedTextSource {
    pub id: Uuid,
    pub conversation: Uuid,
    pub turn: Uuid,
    pub document: NormalizedDocument,
    pub selection: TextScope,
    pub guidance: Option<String>,
}
impl NormalizedTextSource {
    pub fn new(
        conversation: Uuid,
        turn: Uuid,
        document: NormalizedDocument,
        selection: TextScope,
        guidance: Option<String>,
    ) -> Result<Self> {
        let mut value = Self {
            id: Uuid::nil(),
            conversation,
            turn,
            document,
            selection,
            guidance,
        };
        value.id = value.identity()?;
        value.validate()?;
        Ok(value)
    }
    fn identity(&self) -> Result<Uuid> {
        let hash = digest(&serde_json::to_vec(&(
            self.conversation,
            self.turn,
            &self.document,
            &self.selection,
            &self.guidance,
        ))?);
        Ok(Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:rmb:normalized-text:v1:{hash}").as_bytes(),
        ))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.conversation.is_nil() && !self.turn.is_nil() && self.id == self.identity()?,
            "invalid normalized source identity"
        );
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_WRITER_SOURCE,
            "normalized source exceeds Writer bound"
        );
        ensure!(
            self.guidance
                .as_ref()
                .is_none_or(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0')),
            "invalid explicit guidance"
        );
        Index::build(self.document.clone())?;
        let selected = self.selected();
        ensure!(
            !selected.is_empty() && selected.iter().any(|unit| !unit.text.trim().is_empty()),
            "empty source selection"
        );
        Ok(())
    }
    pub fn selected(&self) -> Vec<&crate::retrieval::SourceUnit> {
        self.document
            .units
            .iter()
            .filter(|unit| match self.selection {
                TextScope::Page { page } => unit.page == Some(page),
                TextScope::SuppliedNotebook => true,
            })
            .collect()
    }
}

pub(crate) struct WriterPreparation {
    pub fresh: bool,
    pub root: ExpectedHeads,
    pub guards: Vec<Envelope>,
}

impl Ledger {
    pub(crate) fn writer_preparation_current(
        &self,
        conversation: Uuid,
        prepared: &WriterPreparation,
    ) -> Result<bool> {
        self.complete_store()?;
        let heads = self.store.snapshot_heads_matching(
            &[Namespace::Conversation, Namespace::Source],
            MAX_ITEMS,
            |e| {
                (e.namespace == Namespace::Conversation && e.record_id == conversation)
                    || prepared.guards.iter().any(|guard| {
                        guard.namespace == e.namespace && guard.record_id == e.record_id
                    })
            },
        )?;
        let current = |namespace, id| {
            heads
                .iter()
                .filter(|e| e.namespace == namespace && e.record_id == id)
                .map(|e| e.revision_id)
                .collect::<BTreeSet<_>>()
        };
        Ok(
            current(Namespace::Conversation, conversation) == prepared.root.0
                && prepared
                    .guards
                    .iter()
                    .all(|guard| current(guard.namespace, guard.record_id) == guard.parents),
        )
    }
    pub(crate) fn writer_turn(&self, id: Uuid, conversation: Uuid) -> Result<Turn> {
        self.live_root(conversation, None)?;
        let e = self
            .store
            .value(Namespace::Conversation, id)?
            .context("Writer turn absent")?;
        let Record::Turn(turn) = Self::decode(&e)? else {
            bail!("not a turn")
        };
        ensure!(
            turn.conversation == conversation && turn.mode == Mode::Writer,
            "foreign Writer turn"
        );
        Ok(turn)
    }
    pub(crate) fn prepare_writer(
        &self,
        operation: Uuid,
        expected: ExpectedHeads,
        mut turn: Turn,
        source: NormalizedTextSource,
        selected: Option<Turn>,
    ) -> Result<WriterPreparation> {
        source.validate()?;
        Self::validate_turn(&turn)?;
        ensure!(
            turn.role == Role::User
                && turn.mode == Mode::Writer
                && turn.outcome == Outcome::Interpreted
                && turn.sequence == 0
                && turn.sources == vec![source.id]
                && turn.conversation == source.conversation,
            "invalid Writer preparation"
        );
        let fingerprint = Self::fingerprint(&(
            "prepare-writer-v1",
            &turn,
            &source,
            selected.as_ref().map(|draft| draft.id),
        ))?;
        if let Some(prior) = self.replay(operation, &fingerprint)? {
            return Ok(WriterPreparation {
                fresh: false,
                root: ExpectedHeads(BTreeSet::from([prior.historical.applied_root_revision])),
                guards: vec![],
            });
        }
        let (_, mut root) = self.live_root(turn.conversation, Some(&expected))?;
        // As with allocated sequence, publication time is assigned only after
        // replay comparison; retry fingerprints keep their stable input shape.
        turn.created_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        turn.updated_ms = turn.created_ms;
        let mut records = Vec::new();
        let source_parents = if source.turn == turn.id {
            ExpectedHeads::default()
        } else {
            let current = self
                .store
                .value(Namespace::Source, source.id)?
                .context("source absent")?;
            ensure!(
                matches!(Self::decode(&current)?, Record::NormalizedText(old) if *old == source),
                "source changed"
            );
            ExpectedHeads(BTreeSet::from([current.revision_id]))
        };
        records.push(self.envelope(
            Namespace::Source,
            source.id,
            source_parents,
            Record::NormalizedText(Box::new(source.clone())),
            vec![],
        )?);
        if let Some(selected) = selected {
            let id = selected.id;
            let current = self
                .store
                .value(Namespace::Conversation, id)?
                .context("selected draft absent")?;
            let draft = self.writer_turn(id, turn.conversation)?;
            ensure!(
                draft == selected
                    && draft.role == Role::Assistant
                    && draft.outcome == Outcome::Generated
                    && draft.sources == vec![source.id],
                "invalid selected draft"
            );
            records.push(self.envelope(
                Namespace::Conversation,
                id,
                ExpectedHeads(BTreeSet::from([current.revision_id])),
                Record::Turn(draft),
                vec![],
            )?);
        }
        self.validate_correction_target(&turn)?;
        turn.sequence = root.next_sequence;
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("sequence exhausted")?;
        root.updated_ms = turn.updated_ms;
        let guards = records
            .iter()
            .map(|e| {
                let mut guard = e.clone();
                guard.parents = BTreeSet::from([e.revision_id]);
                guard.revision_id = Uuid::new_v4();
                guard.operation_id = Uuid::new_v4();
                guard
            })
            .collect();
        records.push(self.envelope(
            Namespace::Conversation,
            turn.id,
            ExpectedHeads::default(),
            Record::Turn(turn),
            vec![],
        )?);
        let binding = root.binding;
        let root_envelope = self.envelope(
            Namespace::Conversation,
            root.id,
            expected,
            Record::Root(root),
            vec![],
        )?;
        let root_heads = ExpectedHeads(BTreeSet::from([root_envelope.revision_id]));
        let (_, fresh) =
            self.publish_observed(operation, fingerprint, root_envelope, records, binding)?;
        Ok(WriterPreparation {
            fresh,
            root: root_heads,
            guards,
        })
    }
    pub(crate) fn finish_writer(
        &self,
        operation: Uuid,
        prepared: WriterPreparation,
        mut turn: Turn,
    ) -> Result<(WriteResult, Option<WriterSelection>)> {
        ensure!(
            prepared.fresh
                && turn.role == Role::Assistant
                && turn.mode == Mode::Writer
                && matches!(
                    turn.outcome,
                    Outcome::Generated | Outcome::Failed | Outcome::Canceled
                ),
            "invalid Writer finish"
        );
        Self::validate_turn(&turn)?;
        let fingerprint = Self::fingerprint(&("finish-writer-v1", &turn))?;
        let (_, mut root) = self.live_root(turn.conversation, Some(&prepared.root))?;
        turn.sequence = root.next_sequence;
        root.next_sequence = root
            .next_sequence
            .checked_add(1)
            .context("sequence exhausted")?;
        root.updated_ms = turn.updated_ms;
        let generated = turn.outcome == Outcome::Generated;
        let source = prepared
            .guards
            .iter()
            .find(|e| e.namespace == Namespace::Source)
            .cloned();
        let mut records = prepared.guards;
        let draft = self.envelope(
            Namespace::Conversation,
            turn.id,
            ExpectedHeads::default(),
            Record::Turn(turn),
            vec![],
        )?;
        records.push(draft.clone());
        let binding = root.binding;
        let root_envelope = self.envelope(
            Namespace::Conversation,
            root.id,
            prepared.root,
            Record::Root(root),
            vec![],
        )?;
        let selection = if generated {
            Some(WriterSelection::published(
                &root_envelope,
                &draft,
                &source.context("Writer source guard absent")?,
            )?)
        } else {
            None
        };
        let (result, fresh) =
            self.publish_observed(operation, fingerprint, root_envelope, records, binding)?;
        Ok((result, if fresh { selection } else { None }))
    }
}
