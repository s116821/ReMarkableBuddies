//! Current selected associations are historical evidence, never effect permits.
use super::{
    BindingReceipt, DocumentOwnership, Ledger, Record, SelectedAdmission, SelectedDomainProjection,
};
use crate::storage::selection::SelectionToken;
use crate::storage::{Namespace, Uuid};
use anyhow::{ensure, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectedBindingUnresolved {
    RootNotSelected,
    RootDeleted,
    BindingNotSelected,
    BindingDeleted,
    RootUnbound,
    RootBoundElsewhere,
    OtherConversation,
}

/// No public constructor or mutable access; a saved result never holds admission.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectedBindingCandidate {
    selection: SelectionToken,
    root_revision: Uuid,
    binding_revision: Uuid,
    receipt: BindingReceipt,
}
impl SelectedBindingCandidate {
    pub fn selection(&self) -> &SelectionToken {
        &self.selection
    }
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
pub enum SelectedBindingInspection {
    HistoricalCandidate(Box<SelectedBindingCandidate>),
    Unresolved(SelectedBindingUnresolved),
}

impl SelectedAdmission {
    /// Read only the accepted current selected closure. Missing heads are not
    /// record-key absence coverage, and never imply an unclaimed native page.
    pub fn inspect_buddy_binding(
        &self,
        accepted: &SelectionToken,
        conversation: Uuid,
        document: Uuid,
        page: Uuid,
    ) -> Result<SelectedBindingInspection> {
        use SelectedBindingInspection::*;
        use SelectedBindingUnresolved::*;
        ensure!(
            !conversation.is_nil() && !document.is_nil() && !page.is_nil(),
            "nil binding query identity"
        );
        self.with_current(accepted, |snapshot| {
            let projection = SelectedDomainProjection::from_snapshot(snapshot)?;
            let ownership = projection.document_ownership()?;
            let Some((root_envelope, root)) =
                projection.head(Namespace::Conversation, conversation)
            else {
                return Ok(Unresolved(RootNotSelected));
            };
            let Some(root) = root else {
                return Ok(Unresolved(RootDeleted));
            };
            let Record::Root(root) = root else {
                anyhow::bail!("selected binding query root variant mismatch");
            };
            // Preserve complete live chronology, even when matching page IDs exist.
            projection.inspect(conversation)?;
            let binding_id = Ledger::binding_id(document, page);
            let Some((binding_envelope, binding)) =
                projection.head(Namespace::Conversation, binding_id)
            else {
                return Ok(Unresolved(BindingNotSelected));
            };
            let Some(binding) = binding else {
                return Ok(Unresolved(BindingDeleted));
            };
            let Record::Binding(binding) = binding else {
                anyhow::bail!("selected binding query binding variant mismatch");
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
                binding.receipt.conversation == conversation
                    && binding.receipt.observed_document == document
                    && binding.receipt.observed_page == page
                    && binding.id == binding_id,
                "selected binding query identity mismatch"
            );
            Ledger::validate_binding_receipt(&binding.receipt)?;
            ensure!(
                ownership.get(&conversation) == Some(&DocumentOwnership::Document(document)),
                "selected binding document ownership unresolved"
            );
            Ok(HistoricalCandidate(Box::new(SelectedBindingCandidate {
                selection: snapshot.token.clone(),
                root_revision: root_envelope.revision_id,
                binding_revision: binding_envelope.revision_id,
                receipt: binding.receipt.clone(),
            })))
        })
    }
}
