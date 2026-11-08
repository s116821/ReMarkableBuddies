//! Domain-4 records in the same selected Reader transaction and original receipt.
use super::*;
use crate::conversation::target::{
    Action, Candidate, RecordRef, SourceTarget, TargetState, TargetUse,
};
fn reference(projection: &SelectedDomainProjection, id: Uuid) -> Result<RecordRef> {
    let pin = projection.current_reference(Namespace::Source, id)?;
    Ok(RecordRef {
        namespace: pin.namespace,
        record: pin.record_id,
        revision: pin.revision_id,
        object: pin.object,
    })
}
fn pin(envelope: &Envelope) -> Result<RecordRef> {
    let p = pending::pin(envelope)?;
    Ok(RecordRef {
        namespace: p.namespace,
        record: p.record_id,
        revision: p.revision_id,
        object: p.object,
    })
}
fn parent(projection: &SelectedDomainProjection, id: Uuid) -> BTreeSet<Uuid> {
    projection
        .head(Namespace::Conversation, id)
        .map(|(e, _)| BTreeSet::from([e.revision_id]))
        .unwrap_or_default()
}
pub(super) fn records(
    projection: &SelectedDomainProjection,
    request: &SelectedReaderWrite,
    written: &Turn,
    actor: Uuid,
) -> Result<Vec<Envelope>> {
    match &request.kind {
        SelectedReaderText::InterpretTarget { candidate } => {
            interpret(projection, request, written, actor, candidate)
        }
        SelectedReaderText::Generate { .. } => {
            let id = TargetUse::id(request.user.record_id);
            let Some((old, Some(Record::TargetUse(used)))) =
                projection.head(Namespace::Conversation, id)
            else {
                return Ok(vec![]);
            };
            let Some(about) = &used.about else {
                return Ok(vec![]);
            };
            ensure!(
                request.text.starts_with(&format!("{about}\n")),
                "selected target answer lost About"
            );
            let mut used = (**used).clone();
            let hash = digest(request.text.as_bytes());
            ensure!(
                used.inline_body_sha256
                    .as_ref()
                    .is_none_or(|prior| *prior == hash),
                "selected target answer body changed"
            );
            used.inline_body_sha256 = Some(hash);
            Ok(vec![record(
                actor,
                request,
                BTreeSet::from([old.revision_id]),
                Record::TargetUse(Box::new(used)),
            )?])
        }
        _ => Ok(vec![]),
    }
}
fn interpret(
    projection: &SelectedDomainProjection,
    request: &SelectedReaderWrite,
    user: &Turn,
    actor: Uuid,
    candidate: &Candidate,
) -> Result<Vec<Envelope>> {
    candidate.validate()?;
    ensure!(user.sources.len() == 1, "selected target source ambiguous");
    let source_id = user.sources[0];
    let reference = reference(projection, source_id)?;
    let (_, Some(source)) = projection
        .head(Namespace::Source, source_id)
        .context("selected target original absent")?
    else {
        bail!("selected target original deleted")
    };
    let (source, conversation) = target_source::descriptor(reference, source)?;
    ensure!(
        conversation == user.conversation && source.turn == user.id,
        "selected target original user differs"
    );
    let target = SourceTarget {
        id: Uuid::new_v5(&user.id, b"urn:rmb:source-target:v1"),
        conversation,
        introduced_by: user.id,
        source,
        candidate: candidate.clone(),
        previous: None,
    };
    let target = record(
        actor,
        request,
        BTreeSet::new(),
        Record::SourceTarget(Box::new(target)),
    )?;
    let state_id = TargetState::id(conversation);
    let used = TargetUse {
        id: TargetUse::id(user.id),
        conversation,
        turn: user.id,
        action: if projection.head(Namespace::Conversation, state_id).is_some() {
            Action::Retarget
        } else {
            Action::Introduce
        },
        target: Some(pin(&target)?),
        about: Some(candidate.about()?),
        inline_body_sha256: None,
    };
    let used = record(
        actor,
        request,
        parent(projection, used.id),
        Record::TargetUse(Box::new(used)),
    )?;
    let state = TargetState {
        id: state_id,
        conversation,
        use_reference: pin(&used)?,
    };
    let state = record(
        actor,
        request,
        parent(projection, state.id),
        Record::TargetState(Box::new(state)),
    )?;
    Ok(vec![target, used, state])
}
