//! Eligibility of ordinary settled history; no native operation or migration.
use super::*;
use crate::{
    conversation_maintenance::{Binding as ExpectedBinding, Expected, Summary},
    storage::conversation_cleanup::Snapshot,
};
pub(crate) fn eligible(
    snapshot: &Snapshot,
    target: Uuid,
    budget: &mut crate::storage::conversation_cleanup::Budget,
) -> Result<(Expected, Option<ExpectedBinding>, Summary)> {
    let mut by_hash = BTreeMap::new();
    for e in &snapshot.history {
        budget.charge(0, 1)?;
        by_hash.insert(
            snapshot
                .raw_hashes
                .get(&e.revision_id)
                .context("raw envelope identity")?
                .clone(),
            e,
        );
    }
    let mut membership = BTreeMap::<String, Vec<&crate::storage::Manifest>>::new();
    for m in &snapshot.manifests {
        for r in &m.records {
            budget.charge(0, 1)?;
            membership.entry(r.sha256.clone()).or_default().push(m);
        }
    }
    let mut decoded = BTreeMap::new();
    let mut heads = BTreeMap::<(Namespace, Uuid), Vec<&Envelope>>::new();
    for e in &snapshot.history {
        budget.charge(0, 1)?;
        ensure!(
            e.domain_schema_version == SCHEMA,
            "selected/future history excluded"
        );
        if e.kind == Kind::Value {
            let r = Ledger::decode(e)?;
            let payload = &e.payload["record"];
            let belongs = match &r {
                Record::Root(r) => r.id == target,
                Record::Receipt(r) => r.acknowledgment.conversation == target,
                _ => payload["conversation"].as_str() == Some(target.to_string().as_str()),
            };
            ensure!(belongs, "foreign target history");
            match &r {
                Record::Receipt(r) => {
                    ensure!(r.admitted_intent.is_none(), "selected intent excluded")
                }
                Record::OutcomeFact(r) => ensure!(r.settlement.is_none(), "settlement excluded"),
                Record::Export(r) => ensure!(r.attempt.is_none(), "export attempt excluded"),
                _ => {}
            }
            decoded.insert(e.revision_id, r);
        }
    }
    for e in &snapshot.current {
        heads.entry((e.namespace, e.record_id)).or_default().push(e);
    }
    ensure!(
        heads.values().all(|h| h.len() == 1),
        "conflicted descendant"
    );
    let root_e = *heads
        .get(&(Namespace::Conversation, target))
        .and_then(|v| v.first())
        .context("root absent")?;
    let Record::Root(root) = decoded
        .get(&root_e.revision_id)
        .context("live root required")?
    else {
        bail!("invalid root")
    };
    ensure!(root.id == target, "foreign root");
    let mut binding = None;
    let mut turns = Vec::new();
    let mut summary = Summary::default();
    for e in &snapshot.current {
        if e.kind == Kind::Tombstone {
            continue;
        }
        let r = decoded
            .get(&e.revision_id)
            .context("current domain record")?;
        match r {
            Record::Root(r) => ensure!(r.id == target, "foreign root"),
            Record::Binding(b) => {
                ensure!(
                    b.conversation == target && Some(b.id) == root.binding && binding.is_none(),
                    "foreign/extra binding"
                );
                binding = Some(ExpectedBinding {
                    id: b.id.to_string(),
                    revision: e.revision_id.to_string(),
                    sha256: crate::storage::digest(&serde_json::to_vec(e)?),
                });
            }
            Record::Turn(t) => {
                ensure!(t.conversation == target, "foreign turn");
                turns.push((e, t));
                summary.turns += 1;
            }
            Record::Source(s) => {
                ensure!(s.conversation == target, "foreign source");
                summary.source_records += 1;
            }
            Record::Export(a) => {
                ensure!(
                    a.conversation == target && a.outcome == Outcome::Completed,
                    "unfinished export"
                );
                summary.export_associations += 1;
            }
            _ => {}
        }
    }
    ensure!(
        root.binding.is_some() == binding.is_some(),
        "binding absent"
    );
    let sequences: BTreeSet<_> = turns.iter().map(|(_, t)| t.sequence).collect();
    ensure!(
        sequences.len() == turns.len()
            && sequences.len() as u64 == root.next_sequence
            && sequences.iter().all(|n| *n < root.next_sequence),
        "incomplete chronology"
    );
    let current_sources: BTreeMap<_, _> = snapshot
        .current
        .iter()
        .filter_map(|e| match decoded.get(&e.revision_id) {
            Some(Record::Source(source)) => Some((source.id, source)),
            _ => None,
        })
        .collect();
    for (_, turn) in &turns {
        for source in &turn.sources {
            budget.charge(0, 1)?;
            let source = current_sources.get(source).context("turn source absent")?;
            Ledger::validate_source(source, turn)?;
        }
    }
    for source in current_sources.values() {
        budget.charge(0, 1)?;
        let (_, turn) = turns
            .iter()
            .find(|(_, t)| t.id == source.turn)
            .context("source turn absent")?;
        ensure!(
            turn.sources.contains(&source.id),
            "unreferenced source record"
        );
        Ledger::validate_source(source, turn)?;
    }
    let mut exchanges = BTreeMap::<Uuid, Vec<(&Envelope, &Turn)>>::new();
    for (e, t) in &turns {
        Ledger::validate_turn(t)?;
        exchanges.entry(t.exchange).or_default().push((e, t));
    }
    for exchange in exchanges.values() {
        let users: Vec<_> = exchange
            .iter()
            .filter(|(_, t)| t.role == Role::User)
            .collect();
        let assistants: Vec<_> = exchange
            .iter()
            .filter(|(_, t)| t.role == Role::Assistant)
            .collect();
        ensure!(
            users.len() == 1 && assistants.len() <= 1,
            "ambiguous exchange"
        );
        let (ue, user) = *users[0];
        if let Some((_, a)) = assistants.first() {
            ensure!(
                a.sequence > user.sequence && a.mode == user.mode,
                "foreign response"
            );
        }
        match user.outcome {
            Outcome::Interpreted => ensure!(assistants.len() == 1, "unsettled interpreted request"),
            Outcome::Failed | Outcome::Canceled => {
                terminal(snapshot, ue, user, &by_hash, &membership, budget)?
            }
            _ => bail!("unfinished request"),
        }
        for (e, a) in assistants {
            match a.outcome {
                Outcome::Completed => {}
                Outcome::Failed | Outcome::Canceled => {
                    terminal(snapshot, e, a, &by_hash, &membership, budget)?
                }
                _ => bail!("unfinished response"),
            }
        }
    }
    // Immutable facts are assigned through exact transaction membership and turn ancestry.
    for e in &snapshot.history {
        budget.charge(0, 1)?;
        if let Some(Record::OutcomeFact(f)) = decoded.get(&e.revision_id) {
            let (current, _) = turns
                .iter()
                .find(|(_, t)| t.id == f.turn)
                .context("orphan outcome fact")?;
            let sha = snapshot
                .raw_hashes
                .get(&e.revision_id)
                .context("fact bytes absent")?;
            let ms = membership.get(sha).context("fact manifest absent")?;
            ensure!(ms.len() == 1, "ambiguous fact manifest");
            let mut related = Vec::new();
            for r in &ms[0].records {
                budget.charge(0, 1)?;
                if let Some(x) = by_hash.get(&r.sha256) {
                    if x.record_id == f.turn && x.kind == Kind::Value {
                        related.push(*x);
                    }
                }
            }
            ensure!(related.len() == 1, "ambiguous outcome transaction");
            let original = related[0];
            let Record::Turn(t) = decoded.get(&original.revision_id).context("outcome turn")?
            else {
                bail!("outcome turn shape")
            };
            ensure!(
                t.outcome == f.outcome && t.conversation == f.conversation,
                "contradictory outcome"
            );
            ancestry(snapshot, current, original.revision_id, budget)?;
        }
    }
    Ok((
        Expected {
            revision: root_e.revision_id.to_string(),
            sha256: crate::storage::digest(&serde_json::to_vec(root_e)?),
        },
        binding,
        summary,
    ))
}
fn terminal(
    snapshot: &Snapshot,
    envelope: &Envelope,
    turn: &Turn,
    by_hash: &BTreeMap<String, &Envelope>,
    membership: &BTreeMap<String, Vec<&crate::storage::Manifest>>,
    budget: &mut crate::storage::conversation_cleanup::Budget,
) -> Result<()> {
    let sha = snapshot
        .raw_hashes
        .get(&envelope.revision_id)
        .context("terminal identity")?;
    let ms = membership.get(sha).context("terminal manifest")?;
    ensure!(ms.len() == 1, "ambiguous terminal manifest");
    let mut facts = Vec::new();
    for r in &ms[0].records {
        budget.charge(0, 1)?;
        if let Some(e) = by_hash.get(&r.sha256) {
            if e.kind == Kind::Value {
                if let Record::OutcomeFact(f) = Ledger::decode(e)? {
                    if f.turn == turn.id {
                        facts.push(f);
                    }
                }
            }
        }
    }
    if envelope.parents.len() == 1 {
        if let Some(parent) = snapshot
            .history
            .iter()
            .find(|e| envelope.parents.contains(&e.revision_id))
        {
            if let Record::Turn(old) = Ledger::decode(parent)? {
                if old.outcome == Outcome::ReconcileRequired && turn.outcome == Outcome::Failed {
                    ensure!(
                        facts.len() == 1
                            && matches!(
                                facts[0].reason,
                                AttemptReason::NoSuccessor | AttemptReason::InvalidSuccessor
                            ),
                        "uncertain request not explicitly settled"
                    );
                }
            }
        }
    }
    ensure!(
        facts.len() == 1
            && facts[0].outcome == turn.outcome
            && facts[0].conversation == turn.conversation,
        "terminal proof absent/contradictory"
    );
    Ok(())
}
fn ancestry(
    snapshot: &Snapshot,
    current: &Envelope,
    wanted: Uuid,
    budget: &mut crate::storage::conversation_cleanup::Budget,
) -> Result<()> {
    let by_id: BTreeMap<_, _> = snapshot
        .history
        .iter()
        .map(|e| (e.revision_id, e))
        .collect();
    let mut e = current;
    let mut seen = BTreeSet::new();
    loop {
        budget.charge(0, 1)?;
        ensure!(seen.insert(e.revision_id), "cyclic turn ancestry");
        if e.revision_id == wanted {
            return Ok(());
        }
        ensure!(e.parents.len() == 1, "ambiguous turn ancestry");
        let parent = *by_id
            .get(e.parents.first().context("ancestry absent")?)
            .context("parent absent")?;
        ensure!(
            parent.record_id == current.record_id && parent.namespace == current.namespace,
            "foreign turn parent"
        );
        let Record::Turn(old) = Ledger::decode(parent)? else {
            bail!("invalid parent turn")
        };
        let Record::Turn(new) = Ledger::decode(e)? else {
            bail!("invalid turn")
        };
        ensure!(
            old.id == new.id
                && old.conversation == new.conversation
                && old.exchange == new.exchange
                && old.sequence == new.sequence
                && old.role == new.role
                && old.mode == new.mode
                && old.sources == new.sources
                && old.correction_of == new.correction_of
                && old.created_ms == new.created_ms,
            "immutable turn changed"
        );
        let permitted = matches!(
            (&old.outcome, &new.outcome),
            (
                Outcome::Prepared,
                Outcome::Interpreted
                    | Outcome::Failed
                    | Outcome::Canceled
                    | Outcome::ReconcileRequired
            ) | (
                Outcome::Generated,
                Outcome::Completed
                    | Outcome::Failed
                    | Outcome::Canceled
                    | Outcome::ReconcileRequired
            ) | (
                Outcome::ReconcileRequired,
                Outcome::Failed | Outcome::ReconcileRequired
            )
        );
        ensure!(permitted, "invalid outcome transition");
        if matches!(old.outcome, Outcome::Generated | Outcome::ReconcileRequired) {
            ensure!(old.text == new.text, "draft text changed");
        }
        e = parent;
    }
}

pub(crate) fn original_root(e: &Envelope, target: Uuid) -> Result<Option<Uuid>> {
    ensure!(
        e.namespace == Namespace::Conversation && e.domain_schema_version == SCHEMA,
        "original root schema"
    );
    let Record::Root(root) = Ledger::decode(e)? else {
        bail!("original root shape")
    };
    ensure!(
        root.id == target && e.record_id == target,
        "original root identity"
    );
    Ok(root.binding)
}
pub(crate) fn original_binding(e: &Envelope, target: Uuid, id: Uuid) -> Result<()> {
    ensure!(
        e.namespace == Namespace::Conversation && e.domain_schema_version == SCHEMA,
        "original binding schema"
    );
    let Record::Binding(binding) = Ledger::decode(e)? else {
        bail!("original binding shape")
    };
    ensure!(
        binding.conversation == target && binding.id == id && e.record_id == id,
        "original binding identity"
    );
    Ok(())
}
