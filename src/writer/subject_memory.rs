//! Optional local retrieval snapshots, never selected or native authority.
use super::Failure;
use crate::{memory, storage::Uuid};

pub(super) fn context(
    provider: &dyn memory::Provider,
    request: &str,
    document: Uuid,
    now_ms: u64,
) -> Result<Option<String>, Failure> {
    if request.len() > 2048 {
        return Err(Failure::TooLarge);
    }
    let mut query = memory::Query::new(request.to_owned(), now_ms);
    query.document = Some(document);
    query.validate().map_err(|_| Failure::Storage)?;
    let view = provider.query(&query).map_err(|_| Failure::Storage)?;
    if view.selected() {
        return Err(Failure::Unsupported);
    }
    match view.status {
        memory::Status::Disabled | memory::Status::CompleteEmpty => {
            return if view.context.facts.is_empty() {
                Ok(None)
            } else {
                Err(Failure::Storage)
            };
        }
        memory::Status::Unavailable => return Err(Failure::Storage),
        memory::Status::Complete | memory::Status::Partial => {}
    }
    if view.context.facts.is_empty() {
        return Err(Failure::Storage);
    }
    if view.context.facts.len() > query.max_matches {
        return Err(Failure::TooLarge);
    }
    for item in &view.context.facts {
        item.fact.validate().map_err(|_| Failure::Storage)?;
        if matches!(item.fact.key.scope, memory::Scope::Document { document: owner } if owner != document)
        {
            return Err(Failure::Storage);
        }
    }
    let encoded = view.encoded().map_err(|_| Failure::Storage)?;
    if encoded.len() > query.max_context_bytes {
        return Err(Failure::TooLarge);
    }
    Ok(Some(format!(
        "{}\n{encoded}",
        memory::context::LOCAL_SNAPSHOT_POLICY
    )))
}
