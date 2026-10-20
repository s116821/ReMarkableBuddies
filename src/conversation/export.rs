//! Stable export identity and per-effect facts in the existing association.
use super::*;
use crate::storage::valid_digest;
use serde::{Deserialize, Serialize};

pub const EXPORT_SCHEMA: u32 = 2;

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
                    e.record_id == correlation
                        && e.payload["record"]["attempt"]["id"]
                            .as_str()
                            .and_then(|text| Uuid::parse_str(text).ok())
                            == Some(effect)
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
                        && first
                            .attempt
                            .as_ref()
                            .zip(a.attempt.as_ref())
                            .is_some_and(|(before, after)| before.same_plan(after)),
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
