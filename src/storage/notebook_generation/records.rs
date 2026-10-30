//! Validate notebook links against actual committed or same-publication facts.
use super::*;
use crate::conversation::{
    notebook::{NotebookStageKind, NotebookStageOutcome},
    Ledger, Record,
};

fn reference(index: &Index, records: &[(Envelope, String)], hash: &str) -> Result<Record> {
    let (envelope, _) = records
        .iter()
        .chain(index.records.values())
        .find(|(_, stored)| stored == hash)
        .context("notebook revision is not committed or in this publication")?;
    Ledger::decode(envelope)
}
pub(in crate::storage) fn validate_records(
    root: &Path,
    index: &Index,
    records: &[(Envelope, String)],
) -> Result<()> {
    for (envelope, _) in records {
        if envelope.domain_schema_version != 3 {
            continue;
        }
        match Ledger::decode(envelope)? {
            Record::ArchivedNormalizedSource(_) => (),
            Record::NotebookPlan(plan) => {
                let Record::ArchivedNormalizedSource(archive) =
                    reference(index, records, &plan.source_revision)?
                else {
                    anyhow::bail!("notebook plan archive revision absent")
                };
                let bytes = files::read(
                    &root.join("objects").join(&archive.media.sha256),
                    crate::writer::notebook::MAX_CANONICAL_BYTES as u64,
                )?;
                let source = archive.validate_bytes(&bytes)?;
                plan.validate_source(&archive, &source)?;
            }
            Record::NotebookStage(stage) => {
                let Record::NotebookPlan(plan) = reference(index, records, &stage.plan_revision)?
                else {
                    anyhow::bail!("notebook stage plan revision absent")
                };
                ensure!(
                    plan.id == stage.plan_id
                        && plan.conversation == stage.conversation
                        && plan.operation == stage.operation
                        && plan.core_digest == stage.core_digest
                        && plan.archive_id == stage.archive_id
                        && plan.source_revision == stage.source_revision,
                    "notebook stage plan mismatch"
                );
                match stage.kind {
                    NotebookStageKind::Map => ensure!(
                        plan.map_input_digests.get(stage.ordinal) == Some(&stage.input_digest),
                        "notebook map input mismatch"
                    ),
                    NotebookStageKind::Reduce => {
                        ensure!(
                            stage.predecessors.len() == plan.chunks.len(),
                            "notebook reducer coverage incomplete"
                        );
                        for (ordinal, (id, revision, output_digest)) in
                            stage.predecessors.iter().enumerate()
                        {
                            let Record::NotebookStage(prior) = reference(index, records, revision)?
                            else {
                                anyhow::bail!("notebook reducer result absent")
                            };
                            ensure!(
                                prior.id == *id
                                    && prior.ordinal == ordinal
                                    && prior.kind == NotebookStageKind::Map
                                    && prior.plan_id == stage.plan_id
                                    && prior.plan_revision == stage.plan_revision
                                    && prior.conversation == stage.conversation
                                    && prior.core_digest == stage.core_digest
                                    && prior.outcome == Some(NotebookStageOutcome::Generated)
                                    && prior.observed_output_digest.as_ref() == Some(output_digest),
                                "notebook reducer predecessor mismatch"
                            );
                        }
                    }
                }
                if let Some(prepared) = &stage.prepared_revision {
                    let Record::NotebookStage(prior) = reference(index, records, prepared)? else {
                        anyhow::bail!("notebook result preparation absent")
                    };
                    ensure!(
                        prior.outcome.is_none()
                            && prior.plan_revision == stage.plan_revision
                            && prior.conversation == stage.conversation
                            && prior.prepare_fingerprint()? == stage.prepare_fingerprint()?,
                        "notebook result preparation mismatch"
                    );
                }
            }
            _ => anyhow::bail!("unsupported notebook domain record"),
        }
    }
    Ok(())
}
