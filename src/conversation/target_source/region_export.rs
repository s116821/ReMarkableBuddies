//! Check for competing installed or historical region exports before writing.
use super::{Ledger, Namespace, Record, RegionSource, TargetUse, Uuid};
use crate::conversation::{
    target::{Action, InstalledRegion},
    Outcome,
};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;

impl Ledger {
    pub(crate) fn installed_region_conflict(&self, current: &InstalledRegion) -> Result<bool> {
        let stable = format!(
            "normalized-document:{}:installed-region:{}:",
            current.document, current.page,
        );
        let legacy = format!("normalized-document:{}:target:", current.document);
        let (unavailable, heads) = self.store.snapshot_heads_matching_with_status(
            &[Namespace::ExportAssociation],
            256,
            |entry| {
                entry.payload["record"]["source_scope"]
                    .as_str()
                    .is_some_and(|scope| scope.starts_with(&stable) || scope.starts_with(&legacy))
            },
        )?;
        ensure!(unavailable == 0, "regional export history unavailable");
        let mut installed = BTreeSet::new();
        let mut historical = BTreeSet::new();
        let mut records = BTreeSet::new();
        for head in heads {
            ensure!(
                records.insert(head.record_id),
                "conflicted regional export history"
            );
            let Record::Export(export) = Self::decode(&head)? else {
                anyhow::bail!("unexpected regional export record");
            };
            if matches!(export.outcome, Outcome::Canceled | Outcome::Failed)
                && export.attempt.is_some_and(|attempt| attempt.no_send)
            {
                continue;
            }
            if let Some(id) = export.source_scope.strip_prefix(&stable) {
                let id = Uuid::parse_str(id)?;
                let correlation = export
                    .capacities
                    .map_or(export.id, |attempt| attempt.correlation);
                // A deliberately separate save does not own the normal correlation.
                if id != current.id && correlation == InstalledRegion::correlation(id) {
                    installed.insert((export.conversation, id));
                }
            } else if let Some(id) = export.source_scope.strip_prefix(&legacy) {
                historical.insert((export.conversation, Uuid::parse_str(id)?));
            }
        }

        if installed.is_empty() && historical.is_empty() {
            return Ok(false);
        }
        let (unavailable, heads) =
            self.store
                .snapshot_heads_matching_with_status(&[Namespace::Source], 512, |entry| {
                    let record = &entry.payload["record"];
                    let uuid = |field: &serde_json::Value| {
                        field.as_str().and_then(|value| Uuid::parse_str(value).ok())
                    };
                    uuid(&record["conversation"]).is_some_and(|conversation| {
                        uuid(&record["installed_region"]["id"])
                            .is_some_and(|id| installed.contains(&(conversation, id)))
                            || uuid(&record["target"]["record"])
                                .is_some_and(|id| historical.contains(&(conversation, id)))
                    })
                })?;
        ensure!(unavailable == 0, "regional export provenance unavailable");
        let mut installed_proven = BTreeSet::new();
        let mut historical_proven = BTreeSet::new();
        records.clear();
        for head in heads {
            ensure!(
                records.insert(head.record_id),
                "conflicted regional export source"
            );
            let Record::RegionSource(source) = Self::decode(&head)? else {
                anyhow::bail!("unexpected region provenance record");
            };
            source.validate()?;
            if let Some(prior) = &source.installed_region {
                let key = (source.conversation, prior.id);
                if installed.contains(&key) {
                    if prior.dimensions != current.dimensions
                        || current.geometry.overlaps(&prior.geometry)?
                    {
                        return Ok(true);
                    }
                    installed_proven.insert(key);
                }
            }
            let key = (source.conversation, source.target.record);
            if historical.contains(&key) {
                if self.historical_region_may_overlap(&source, current)? {
                    return Ok(true);
                }
                historical_proven.insert(key);
            }
        }
        Ok(installed != installed_proven || historical != historical_proven)
    }

    fn historical_region_may_overlap(
        &self,
        prior: &RegionSource,
        current: &InstalledRegion,
    ) -> Result<bool> {
        let used = TargetUse {
            id: TargetUse::id(prior.turn),
            conversation: prior.conversation,
            turn: prior.turn,
            action: Action::Inherit,
            target: Some(prior.target.clone()),
            about: None,
            inline_body_sha256: None,
        };
        let view = self
            .target_views(&[used])?
            .pop()
            .context("historical export target absent")?;
        let Record::ResidentCapture(capture) = view.source else {
            return Ok(true);
        };
        ensure!(
            capture.before.document == current.document,
            "historical export source document differs"
        );
        if capture.before.page != current.page {
            return Ok(false);
        }
        let geometry = view
            .target
            .candidate
            .geometry
            .context("historical export geometry absent")?;
        Ok(prior.dimensions != current.dimensions || current.geometry.overlaps(&geometry)?)
    }
}
