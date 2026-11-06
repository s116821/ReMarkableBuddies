//! Frozen ordinary context; no publication or freshness authority.
use super::{LocalMemory, Provider, Query, Status};
use crate::storage::{Store, Uuid};
use anyhow::{ensure, Result};

#[derive(Clone)]
pub(crate) struct OrdinarySnapshot {
    part: Option<String>,
}

impl OrdinarySnapshot {
    pub(crate) fn query(
        provider: &LocalMemory,
        store: &Store,
        request: &str,
        document: Option<Uuid>,
        now_ms: u64,
    ) -> Result<Self> {
        provider.admit_ordinary_writer(store)?;
        ensure!(
            document.is_none_or(|id| !id.is_nil()),
            "invalid memory document"
        );
        let mut query = Query::new(request.to_owned(), now_ms);
        query.document = document;
        query.validate()?;
        let result = provider.query(&query)?;
        crate::conversation::receiving::memory::validate_result(&result, provider.profile, &query)?;
        ensure!(
            result.status != Status::Partial || !result.context.facts.is_empty(),
            "unusable partial memory"
        );
        let part = if result.status == Status::CompleteEmpty {
            None
        } else {
            Some(format!(
                "{}\n{}",
                super::context::LOCAL_SNAPSHOT_POLICY,
                result.encoded()?
            ))
        };
        Ok(Self { part })
    }

    pub(crate) fn part(&self) -> Option<&str> {
        self.part.as_deref()
    }
}
