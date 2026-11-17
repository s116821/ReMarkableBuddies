//! Stable export identity and per-effect facts in the existing association.
use super::*;
use crate::storage::valid_digest;
use serde::{Deserialize, Serialize};

pub const EXPORT_SCHEMA: u32 = 2;

pub(crate) const CAPACITIES_BACKEND: &str = "capacities";
pub(crate) const CAPACITIES_PAGE: &str = "RootPage";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapacitiesAttempt {
    pub evidence_version: u32,
    pub id: Uuid,
    pub space: Uuid,
    pub structure: String,
    pub correlation: Uuid,
    pub payload_sha256: String,
    pub body_sha256: String,
    pub tags: BTreeSet<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<Uuid>,
    pub candidate: Option<Uuid>,
    pub verified_body_block: Option<String>,
}

impl CapacitiesAttempt {
    pub fn association_id(conversation: Uuid, space: Uuid, correlation: Uuid) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "urn:remarkable-buddies:export:capacities:v1:{space}:{conversation}:{correlation}"
            )
            .as_bytes(),
        )
    }

    pub(crate) fn stage_id(effect: Uuid, stage: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("urn:remarkable-buddies:capacities-export-command:v1:{effect}:{stage}")
                .as_bytes(),
        )
    }

    fn same_plan(&self, other: &Self) -> bool {
        self.evidence_version == other.evidence_version
            && self.id == other.id
            && self.space == other.space
            && self.structure == other.structure
            && self.correlation == other.correlation
            && self.payload_sha256 == other.payload_sha256
            && self.body_sha256 == other.body_sha256
            && self.tags == other.tags
            && self.grant_id == other.grant_id
    }

    fn validate(&self, association: &ExportAssociation) -> Result<()> {
        let stage = match association.outcome {
            Outcome::Prepared if self.candidate.is_none() => "prepared",
            Outcome::ReconcileRequired if self.candidate.is_none() => "uncertain",
            Outcome::ReconcileRequired => "candidate",
            Outcome::Completed => "completed",
            Outcome::Canceled if self.candidate.is_none() => "canceled-no-send",
            Outcome::Failed if self.candidate.is_none() => "failed-no-send",
            _ => bail!("invalid Capacities export stage"),
        };
        ensure!(
            association.backend == CAPACITIES_BACKEND
                && matches!(
                    (self.evidence_version, self.grant_id),
                    (1, None) | (2, Some(_))
                )
                && self.grant_id.is_none_or(|id| !id.is_nil())
                && association.attempt.is_none()
                && self.structure == CAPACITIES_PAGE
                && !association.conversation.is_nil()
                && !self.space.is_nil()
                && !self.correlation.is_nil()
                && association.conversation != self.correlation
                && !self.id.is_nil()
                && association.id
                    == Self::association_id(association.conversation, self.space, self.correlation)
                && association.operation == Self::stage_id(self.id, stage)
                && (1..=1024).contains(&association.source_scope.len())
                && (1..=1024).contains(&association.source_revision.len())
                && valid_digest(&self.payload_sha256)
                && valid_digest(&self.body_sha256)
                && self.tags.len() <= 16
                && self.tags.iter().all(|id| !id.is_nil())
                && self.candidate.is_none_or(|id| !id.is_nil()),
            "invalid Capacities export scope"
        );
        if association.outcome == Outcome::Completed {
            ensure!(
                self.candidate.map(|id| id.to_string()) == association.native_note
                    && self.candidate.is_some()
                    && self.verified_body_block.as_ref().is_some_and(|id| {
                        (1..=1024).contains(&id.len()) && !id.chars().any(char::is_control)
                    }),
                "unverified Capacities export completion"
            );
        } else {
            ensure!(
                association.native_note.is_none() && self.verified_body_block.is_none(),
                "unverified Capacities export claims native binding"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExportAction {
    Create,
    Update,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExportBaseline {
    pub body_sha256: String,
    pub version: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExportAttempt {
    /// External effect identity, distinct from each Store transition command.
    pub id: Uuid,
    pub action: ExportAction,
    pub body_sha256: String,
    pub target: String,
    pub baseline: Option<ExportBaseline>,
    pub confirmed_version: Option<u64>,
    /// Positive no-send proof on Canceled/Failed only. False never proves that
    /// an HTTP call ran; Completed records verified desired content, too.
    pub no_send: bool,
}

impl ExportAttempt {
    pub fn stage_id(&self, stage: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!(
                "urn:remarkable-buddies:export-command:v1:{}:{stage}",
                self.id
            )
            .as_bytes(),
        )
    }
    fn same_plan(&self, other: &Self) -> bool {
        self.id == other.id
            && self.action == other.action
            && self.body_sha256 == other.body_sha256
            && self.target == other.target
            && self.baseline == other.baseline
    }
}

impl ExportAssociation {
    pub(crate) fn validate_attempt(&self) -> Result<()> {
        if let Some(attempt) = &self.capacities {
            return attempt.validate(self);
        }
        ensure!(
            self.backend != CAPACITIES_BACKEND,
            "missing Capacities evidence"
        );
        let Some(a) = &self.attempt else {
            return Ok(());
        };
        let stage = match self.outcome {
            Outcome::Prepared => "prepared",
            Outcome::Completed => "completed",
            Outcome::Failed => "failed-no-send",
            Outcome::Canceled => "canceled-no-send",
            Outcome::ReconcileRequired => "uncertain",
            _ => bail!("invalid export stage"),
        };
        ensure!(
            self.operation == a.stage_id(stage),
            "export stage command changed"
        );
        ensure!(
            !self.id.is_nil()
                && !self.conversation.is_nil()
                && !self.operation.is_nil()
                && !a.id.is_nil()
                && valid_digest(&a.body_sha256)
                && (1..=128).contains(&self.backend.len())
                && (1..=1024).contains(&self.source_scope.len())
                && (1..=1024).contains(&self.source_revision.len())
                && (1..=1024).contains(&a.target.len())
                && !a.target.chars().any(char::is_control),
            "invalid export attempt scope"
        );
        match a.action {
            ExportAction::Create => ensure!(a.baseline.is_none(), "create has update baseline"),
            ExportAction::Update => {
                let baseline = a.baseline.as_ref().context("update lacks baseline")?;
                ensure!(
                    valid_digest(&baseline.body_sha256) && baseline.version > 0,
                    "invalid export baseline"
                );
                ensure!(
                    self.native_note.as_deref() == Some(&a.target),
                    "update target drift"
                );
            }
        }
        match self.outcome {
            Outcome::Prepared | Outcome::ReconcileRequired => ensure!(
                !a.no_send && a.confirmed_version.is_none(),
                "unconfirmed attempt claims terminal facts"
            ),
            Outcome::Completed => {
                let version = a
                    .confirmed_version
                    .context("completed export lacks version")?;
                ensure!(
                    !a.no_send && version > 0 && self.native_note.as_deref() == Some(&a.target),
                    "invalid completed export identity"
                );
                if let Some(baseline) = &a.baseline {
                    ensure!(
                        version >= baseline.version
                            && (a.body_sha256 == baseline.body_sha256
                                || version > baseline.version),
                        "inconsistent confirmed export version"
                    );
                }
            }
            Outcome::Failed | Outcome::Canceled => ensure!(
                a.no_send && a.confirmed_version.is_none(),
                "no-effect terminal lacks positive no-send"
            ),
            _ => bail!("invalid export attempt outcome"),
        }
        if a.action == ExportAction::Create && self.outcome != Outcome::Completed {
            ensure!(
                self.native_note.is_none(),
                "unconfirmed create claims native note"
            );
        }
        Ok(())
    }

    pub(crate) fn validate_successor(&self, old: &Self) -> Result<()> {
        ensure!(
            old.conversation == self.conversation
                && old.backend == self.backend
                && old.source_scope == self.source_scope,
            "export immutable scope changed"
        );
        if self.capacities.is_some() || old.capacities.is_some() {
            let (before, after) = old
                .capacities
                .as_ref()
                .zip(self.capacities.as_ref())
                .context("Capacities schema changed")?;
            ensure!(
                before.correlation == after.correlation
                    && before.space == after.space
                    && before.structure == after.structure,
                "Capacities destination changed"
            );
            if before.id == after.id {
                ensure!(
                    before.same_plan(after) && old.source_revision == self.source_revision,
                    "Capacities attempt plan changed"
                );
                let valid = match (&old.outcome, &self.outcome) {
                    (Outcome::Prepared, Outcome::Canceled | Outcome::Failed) => true,
                    (Outcome::Prepared, Outcome::ReconcileRequired) => true,
                    (Outcome::ReconcileRequired, Outcome::Completed) => {
                        before.candidate.is_some() && before.candidate == after.candidate
                    }
                    _ => false,
                };
                ensure!(valid, "invalid Capacities export transition");
            } else {
                ensure!(
                    matches!(old.outcome, Outcome::Canceled | Outcome::Failed)
                        && self.outcome == Outcome::Prepared,
                    "Capacities attempt cannot start another create"
                );
            }
            return Ok(());
        }
        match (&old.attempt, &self.attempt) {
            (None, None) => {
                ensure!(
                    old.source_revision == self.source_revision,
                    "export immutable scope changed"
                );
                ensure!(
                    matches!(old.outcome, Outcome::Prepared | Outcome::ReconcileRequired),
                    "export already terminal"
                );
            }
            (Some(before), Some(after)) if before.id == after.id => {
                ensure!(
                    before.same_plan(after) && old.source_revision == self.source_revision,
                    "export attempt plan changed"
                );
                ensure!(
                    matches!(old.outcome, Outcome::Prepared | Outcome::ReconcileRequired),
                    "export attempt already terminal"
                );
                ensure!(
                    old.outcome != Outcome::ReconcileRequired || !after.no_send,
                    "uncertain effect cannot become no-send"
                );
                ensure!(
                    self.native_note == old.native_note
                        || (before.action == ExportAction::Create
                            && self.outcome == Outcome::Completed
                            && self.native_note.as_deref() == Some(&before.target)),
                    "export target changed"
                );
            }
            (Some(before), Some(after)) => {
                ensure!(
                    old.outcome == Outcome::Completed
                        || (matches!(old.outcome, Outcome::Canceled | Outcome::Failed)
                            && before.no_send),
                    "unresolved export cannot start successor"
                );
                ensure!(
                    self.outcome == Outcome::Prepared && self.native_note == old.native_note,
                    "invalid new export cycle"
                );
                let baseline = if old.outcome == Outcome::Completed {
                    Some(ExportBaseline {
                        body_sha256: before.body_sha256.clone(),
                        version: before
                            .confirmed_version
                            .context("prior confirmed version absent")?,
                    })
                } else {
                    before.baseline.clone()
                };
                ensure!(
                    after.baseline == baseline,
                    "new export lost confirmed baseline"
                );
                if let Some(note) = &old.native_note {
                    ensure!(
                        after.action == ExportAction::Update && &after.target == note,
                        "successor changed confirmed destination"
                    );
                } else {
                    ensure!(
                        after.action == ExportAction::Create,
                        "unbound successor is not create"
                    );
                }
            }
            _ => {
                bail!("legacy export requires explicit verified adoption; schema downgrade refused")
            }
        }
        Ok(())
    }
}

impl Ledger {
    /// Resolve the portable correlation against existing heads, including all
    /// conflicting heads of a matching record. No active provider preference.
    pub(crate) fn original_exports(
        &self,
        conversation: Uuid,
        correlation: Uuid,
    ) -> Result<Vec<ExportAssociation>> {
        ensure!(
            !conversation.is_nil() && !correlation.is_nil() && conversation != correlation,
            "invalid export identity"
        );
        self.live_root(conversation, None)?;
        let (unavailable, heads) = self.store.snapshot_heads_matching_with_status(
            &[Namespace::ExportAssociation],
            8,
            |envelope| {
                let record = &envelope.payload["record"];
                let matches_uuid = |field: &serde_json::Value, id| {
                    field.as_str().and_then(|s| Uuid::parse_str(s).ok()) == Some(id)
                };
                envelope.record_id == correlation
                    || (matches_uuid(&record["conversation"], conversation)
                        && matches_uuid(&record["capacities"]["correlation"], correlation))
            },
        )?;
        ensure!(unavailable == 0, "incomplete export history");
        heads
            .iter()
            .map(|head| {
                let Record::Export(association) = Self::decode(head)? else {
                    bail!("invalid original export")
                };
                ensure!(
                    association.conversation == conversation
                        && association
                            .capacities
                            .as_ref()
                            .map_or(association.id, |a| a.correlation)
                            == correlation,
                    "foreign export association"
                );
                Ok(association)
            })
            .collect()
    }

    /// Current export facts only; no dispatch or native capability is returned.
    pub fn export_association(
        &self,
        conversation: Uuid,
        id: Uuid,
    ) -> Result<Option<ExportAssociation>> {
        self.complete_store()?;
        self.live_root(conversation, None)?;
        ensure!(
            !id.is_nil() && id != conversation,
            "invalid export identity"
        );
        let mut heads = self.store.heads(Namespace::ExportAssociation, id)?;
        ensure!(heads.len() <= 1, "conflicted export association");
        let Some(envelope) = heads.pop() else {
            return Ok(None);
        };
        let Record::Export(export) = Self::decode(&envelope)? else {
            bail!("not an export association");
        };
        ensure!(
            export.conversation == conversation,
            "foreign export conversation"
        );
        Ok(Some(export))
    }

    /// Bounded historical attempt lookup using retained revisions and its
    /// original stage receipt; never restores a deleted/current association.
    pub fn historical_export_attempt(
        &self,
        conversation: Uuid,
        correlation: Uuid,
        effect: Uuid,
    ) -> Result<Option<ExportAssociation>> {
        self.complete_store()?;
        self.live_root(conversation, None)?;
        ensure!(!effect.is_nil(), "nil export effect");
        let snapshot =
            self.store
                .snapshot_revisions_matching(&[Namespace::ExportAssociation], 8, |e| {
                    let payload = &e.payload["record"];
                    e.record_id == correlation
                        && [&payload["attempt"]["id"], &payload["capacities"]["id"]]
                            .into_iter()
                            .any(|id| {
                                id.as_str().and_then(|s| Uuid::parse_str(s).ok()) == Some(effect)
                            })
                })?;
        if snapshot.is_empty() {
            return Ok(None);
        }
        let mut heads: BTreeSet<_> = snapshot.iter().map(|e| e.revision_id).collect();
        let mut plan: Option<ExportAssociation> = None;
        for e in &snapshot {
            let Record::Export(a) = Self::decode(e)? else {
                bail!("invalid historical export");
            };
            ensure!(
                a.conversation == conversation && a.id == correlation,
                "foreign historical export"
            );
            if let Some(first) = &plan {
                ensure!(
                    first.backend == a.backend
                        && first.source_scope == a.source_scope
                        && first.source_revision == a.source_revision
                        && (first
                            .attempt
                            .as_ref()
                            .zip(a.attempt.as_ref())
                            .is_some_and(|(before, after)| before.same_plan(after))
                            || first
                                .capacities
                                .as_ref()
                                .zip(a.capacities.as_ref())
                                .is_some_and(|(before, after)| before.same_plan(after))),
                    "historical export plan conflict"
                );
            } else {
                plan = Some(a);
            }
            for parent in &e.parents {
                heads.remove(parent);
            }
        }
        ensure!(heads.len() == 1, "conflicted historical export attempt");
        let selected = snapshot
            .iter()
            .find(|e| heads.contains(&e.revision_id))
            .context("historical export head missing")?;
        let Record::Export(a) = Self::decode(selected)? else {
            bail!("not an export");
        };
        let fingerprint = Self::fingerprint(&("export", &a))?;
        ensure!(
            self.replay(a.operation, &fingerprint)?.is_some(),
            "historical export receipt missing"
        );
        Ok(Some(a))
    }
}
