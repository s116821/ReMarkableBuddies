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
        Self::from_result(result, provider.profile, &query)
    }

    fn from_result(result: super::QueryResult, profile: Uuid, query: &Query) -> Result<Self> {
        crate::conversation::receiving::memory::validate_result(&result, profile, query)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Basis, Fact, Key, Mutation, Provenance, Scope};
    use std::sync::Arc;

    #[test]
    fn ordinary_snapshot_keeps_scope_bounds_and_refuses_bad_results() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("ordinary-memory-snapshot-{}", Uuid::new_v4()));
        let paths = crate::storage::StorePaths {
            data: root.join("data"),
            cache: root.join("cache"),
            credentials: root.join("credentials"),
        };
        let store = Arc::new(Store::open(paths.clone())?);
        let profile = Uuid::new_v4();
        let provider = LocalMemory::new_local(store.clone(), profile)?;
        let document = Uuid::new_v4();
        for (scope, claim, value) in [
            (Scope::Global, "global", "global physics context"),
            (
                Scope::Document { document },
                "original",
                "original document physics context",
            ),
            (
                Scope::Document {
                    document: Uuid::new_v4(),
                },
                "foreign",
                "foreign document physics context",
            ),
        ] {
            provider.put(Mutation {
                operation: Uuid::new_v4(),
                expected: Default::default(),
                fact: Fact {
                    key: Key {
                        profile,
                        scope,
                        fact: Uuid::new_v4(),
                    },
                    claim: claim.into(),
                    value: value.into(),
                    subject: "physics".into(),
                    tags: vec![],
                    provenance: Provenance {
                        basis: Basis::UserConfirmed,
                        description: "Synthetic caller test".into(),
                        conversation: None,
                        turn: None,
                        source: None,
                    },
                    confidence_basis_points: 1000,
                    observed_ms: 1,
                    expires_ms: None,
                },
            })?;
        }
        let snapshot = OrdinarySnapshot::query(&provider, &store, "physics", Some(document), 2)?;
        let part = snapshot.part().unwrap();
        assert!(
            part.contains("global physics")
                && part.contains("original document physics")
                && !part.contains("foreign document physics")
        );
        let global = OrdinarySnapshot::query(&provider, &store, "physics", None, 2)?;
        assert!(!global.part().unwrap().contains("document physics"));
        let count = provider.query_count();
        assert!(OrdinarySnapshot::query(&provider, &store, &"x".repeat(2049), None, 2).is_err());
        assert_eq!(provider.query_count(), count);
        assert!(
            OrdinarySnapshot::query(&provider, &store, &"x".repeat(2048), None, 2)?
                .part()
                .is_none()
        );
        let mut query = Query::new("physics".into(), 2);
        query.document = Some(document);
        let valid = provider.query(&query)?;
        assert!(OrdinarySnapshot::from_result(valid.clone(), Uuid::new_v4(), &query).is_err());
        let mut bad = valid.clone();
        bad.status = Status::Unavailable;
        assert!(OrdinarySnapshot::from_result(bad, profile, &query).is_err());
        let mut bad = valid.clone();
        bad.status = Status::Partial;
        bad.context.facts.clear();
        bad.diagnostics.unsupported = 1;
        assert!(OrdinarySnapshot::from_result(bad, profile, &query).is_err());
        let mut bad = valid.clone();
        bad.context.facts[0].revision = Uuid::nil();
        assert!(OrdinarySnapshot::from_result(bad, profile, &query).is_err());
        let mut small = query.clone();
        small.max_context_bytes = 256;
        assert!(OrdinarySnapshot::from_result(valid, profile, &small).is_err());
        drop(provider);
        drop(store);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
