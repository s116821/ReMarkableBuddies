//! Atomic historical association facts, never current native effect permission.
use super::{BindingReceipt, IncompleteStore, Ledger, Record};
use crate::storage::{Kind, Namespace, Uuid, MAX_ITEMS};
use anyhow::{ensure, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoricalPageAssociationUnresolved {
    RootMissing,
    RootConflicted,
    RootTombstoned,
    BindingMissing,
    BindingConflicted,
    BindingTombstoned,
    RootUnbound,
    RootBoundElsewhere,
    OtherConversation,
}

/// A cloned historical receipt and the two heads seen in one Store snapshot.
/// No public constructor, mutable receipt, Store admission or effect permit.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoricalPageAssociation {
    root_revision: Uuid,
    binding_revision: Uuid,
    receipt: BindingReceipt,
}

impl HistoricalPageAssociation {
    pub fn root_revision(&self) -> Uuid {
        self.root_revision
    }
    pub fn binding_revision(&self) -> Uuid {
        self.binding_revision
    }
    pub fn receipt(&self) -> &BindingReceipt {
        &self.receipt
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HistoricalPageAssociationInspection {
    HistoricalCandidate(Box<HistoricalPageAssociation>),
    Unresolved(HistoricalPageAssociationUnresolved),
}

impl Ledger {
    /// Read current stored heads once; matching IDs do not establish live state.
    pub fn inspect_page_association(
        &self,
        conversation: Uuid,
        document: Uuid,
        page: Uuid,
    ) -> Result<HistoricalPageAssociationInspection> {
        use HistoricalPageAssociationInspection::*;
        use HistoricalPageAssociationUnresolved::*;
        ensure!(
            !conversation.is_nil() && !document.is_nil() && !page.is_nil(),
            "nil association query identity"
        );
        let binding_id = Self::binding_id(document, page);
        ensure!(
            conversation != binding_id,
            "association query key collision"
        );
        let identity = conversation.to_string();
        let (unavailable_commits, snapshot) = self.store.snapshot_heads_matching_with_status(
            &[
                Namespace::Conversation,
                Namespace::Source,
                Namespace::ExportAssociation,
            ],
            MAX_ITEMS,
            |e| {
                let record = &e.payload["record"];
                (e.namespace == Namespace::Conversation
                    && (e.record_id == conversation || e.record_id == binding_id))
                    || record["conversation"].as_str() == Some(&identity)
                    || record["acknowledgment"]["conversation"].as_str() == Some(&identity)
            },
        )?;
        if unavailable_commits != 0 {
            return Err(IncompleteStore {
                unavailable_commits,
            }
            .into());
        }
        let heads = |id| {
            snapshot
                .iter()
                .filter(|e| e.namespace == Namespace::Conversation && e.record_id == id)
                .collect::<Vec<_>>()
        };
        let roots = heads(conversation);
        if roots.is_empty() {
            return Ok(Unresolved(RootMissing));
        }
        if roots.len() != 1 {
            return Ok(Unresolved(RootConflicted));
        }
        if roots[0].kind == Kind::Tombstone {
            return Ok(Unresolved(RootTombstoned));
        }
        let Record::Root(root) = Self::decode(roots[0])? else {
            anyhow::bail!("association query root variant mismatch");
        };
        let bindings = heads(binding_id);
        if bindings.is_empty() {
            return Ok(Unresolved(BindingMissing));
        }
        if bindings.len() != 1 {
            return Ok(Unresolved(BindingConflicted));
        }
        if bindings[0].kind == Kind::Tombstone {
            return Ok(Unresolved(BindingTombstoned));
        }
        let Record::Binding(binding) = Self::decode(bindings[0])? else {
            anyhow::bail!("association query binding variant mismatch");
        };
        if binding.conversation != conversation {
            return Ok(Unresolved(OtherConversation));
        }
        match root.binding {
            None => return Ok(Unresolved(RootUnbound)),
            Some(id) if id != binding_id => return Ok(Unresolved(RootBoundElsewhere)),
            Some(_) => {}
        }
        ensure!(
            binding.id == binding_id
                && binding.receipt.conversation == conversation
                && binding.receipt.observed_document == document
                && binding.receipt.observed_page == page,
            "historical association identity mismatch"
        );
        Self::validate_binding_receipt(&binding.receipt)?;
        // Preserve the delivered ledger's full chronology/record validation,
        // using only this same snapshot (including any queried foreign head).
        let records = Self::inspect_snapshot(conversation, false, &snapshot)?;
        self.validate_snapshot_evidence(&records, &snapshot)?;
        Ok(HistoricalCandidate(Box::new(HistoricalPageAssociation {
            root_revision: roots[0].revision_id,
            binding_revision: bindings[0].revision_id,
            receipt: binding.receipt,
        })))
    }
}
