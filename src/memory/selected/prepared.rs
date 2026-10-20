use super::*;
use crate::memory::prepared::{self as evidence, Domain, Intent, Pin};
use crate::storage::conditional::Refused;

fn pins(view: &View) -> Vec<Pin> {
    let mut pins = vec![Pin::from_read(&view.catalog_read)];
    pins.extend(view.reads.values().map(Pin::from_read));
    pins.sort_by(|a, b| a.scope.key_sha256.cmp(&b.scope.key_sha256));
    pins
}
fn seal(mut view: View, key: &Key) -> Result<super::super::Inspection> {
    let heads = view
        .aggregates
        .values()
        .flat_map(|a| &a.heads)
        .filter(|e| key.record_id().is_ok_and(|id| id == e.record_id))
        .cloned()
        .collect();
    let mut admission = QueryResult::empty(Status::Complete);
    view.pins.result_sha256 = Some(result_identity(&admission)?);
    admission.pins = Some(Arc::new(Mutex::new(view.pins)));
    super::super::Inspection::new(heads, Some(admission))
}
impl SelectedMemory {
    pub(crate) fn prepare_forget_view(
        &self,
        key: &Key,
    ) -> Result<(Vec<Pin>, super::super::Inspection)> {
        self.own(key)?;
        let view = self.read()?;
        let evidence = pins(&view);
        Ok((evidence, seal(view, key)?))
    }
    fn check_other_pins(&self, view: &View, intent: &Intent, target: &str) -> Result<()> {
        let Domain::Selected {
            identity,
            pins: expected,
        } = &intent.binding.domain
        else {
            return Err(Refused.into());
        };
        if identity != &self.identity {
            return Err(Refused.into());
        }
        let actual = pins(view);
        if actual.len() != expected.len()
            || actual
                .iter()
                .zip(expected)
                .any(|(a, b)| a.scope != b.scope || (a.scope.key_sha256 != target && a != b))
        {
            return Err(Refused.into());
        }
        Ok(())
    }
    pub(crate) fn prepared_forget(
        &self,
        intent: &Intent,
        receipt_only: bool,
        attempted: &mut bool,
    ) -> Result<(Uuid, super::super::Inspection)> {
        self.own(&intent.key)?;
        let envelope = intent.envelope(&self.store)?;
        let owner = match intent.key.scope {
            super::super::Scope::Global => Owner::GlobalFact {
                key: intent.key.clone(),
            },
            super::super::Scope::Document { document } => Owner::Document { document },
        };
        let target = owner.key_sha256()?;
        let mut view = self.read()?;
        self.check_other_pins(&view, intent, &target)?;
        let Domain::Selected { pins: expected, .. } = &intent.binding.domain else {
            return Err(Refused.into());
        };
        let expected_target = expected
            .iter()
            .find(|p| p.scope.key_sha256 == target)
            .ok_or(Refused)?;
        let read = view
            .reads
            .get(&target)
            .context("prepared target registration absent")?;
        let token = read.snapshot.token.clone();
        let lineage: Vec<_> = read
            .accepted_history
            .iter()
            .map(|a| a.transaction.clone())
            .collect();
        let prior = original_receipt(&lineage, &envelope).map_err(|_| Refused)?;
        if let Some(index) = lineage.iter().position(|t| t.operation == intent.operation) {
            let predecessor = read
                .accepted_history
                .get(index + 1)
                .context("prepared predecessor absent")?;
            if !expected_target.matches_history(predecessor)? {
                return Err(Refused.into());
            }
            evidence::verify_parents(intent, &predecessor.selected_records).map_err(|_| Refused)?;
        } else {
            if receipt_only {
                anyhow::bail!("original receipt unresolved")
            }
            if *expected_target != Pin::from_read(read) {
                return Err(Refused.into());
            }
            let heads = &view
                .aggregates
                .get(&target)
                .context("prepared aggregate absent")?
                .heads;
            let current: Vec<_> = heads
                .iter()
                .filter(|p| p.record_id == envelope.record_id)
                .cloned()
                .collect();
            if current
                .iter()
                .map(|e| e.revision_id)
                .collect::<std::collections::BTreeSet<_>>()
                != envelope.parents
            {
                return Err(Refused.into());
            }
            evidence::verify_parents(intent, &current).map_err(|_| Refused)?;
        }
        let transaction = view.pins.guard(&self.store, || {
            if let Some(prior) = prior {
                return Ok(prior);
            }
            // Actual selected CAS still checks this fresh token under the Store mutex.
            *attempted = true;
            Ok(self
                .store
                .commit_selected(&token, intent.operation, vec![envelope], BTreeMap::new())?
                .transaction
                .selected
                .transaction_id)
        })?;
        // Seal a fresh post-operation admission through stdout. Own commit changed the
        // target pin; all other original pins must still be current, not substituted.
        let fresh = self.read()?;
        self.check_other_pins(&fresh, intent, &target)?;
        Ok((transaction, seal(fresh, &intent.key)?))
    }
}
