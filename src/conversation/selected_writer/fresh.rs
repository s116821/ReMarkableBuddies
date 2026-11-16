//! Private pre-target resident frame and Writer interpretation publication.
use super::super::selected_reader_prepare::ResidentPreparedAcknowledgment;
use super::*;

pub(crate) struct FreshFrame {
    capture: IntentRecordRef,
    pub(crate) document: Uuid,
    pub(crate) original: Vec<u8>,
}
impl FreshFrame {
    pub(crate) fn check(
        &self,
        writer: &SelectedAdmission,
        store: &Store,
        token: &SelectionToken,
        root: &IntentRecordRef,
        reads: &mut SelectedReadBudget,
    ) -> Result<()> {
        let current = writer.fresh_writer_frame_locked(store, token, root, &self.capture, reads)?;
        ensure!(
            current.document == self.document && current.original == self.original,
            "fresh Writer frame changed"
        );
        Ok(())
    }
}
impl SelectedAdmission {
    pub(crate) fn fresh_writer_frame_locked(
        &self,
        store: &Store,
        token: &SelectionToken,
        root_ref: &IntentRecordRef,
        capture_ref: &IntentRecordRef,
        reads: &mut SelectedReadBudget,
    ) -> Result<FreshFrame> {
        ensure!(
            std::ptr::eq(store, self.writer_store().as_ref())
                && store.source_target_generation()?.is_some()
                && !store.ordinary_reader_family()?,
            "fresh Writer requires selected target family"
        );
        let mut snapshot = self.writer_snapshot(store, Some(reads))?;
        ensure!(snapshot.token == *token, "fresh Writer selection changed");
        let p = SelectedDomainProjection::from_snapshot(&snapshot)?;
        let root = root(&p, root_ref)?;
        let Record::ResidentCapture(capture) = current(&p, capture_ref)? else {
            bail!("fresh Writer capture variant")
        };
        let Some((_, Some(Record::Turn(user)))) = p.head(Namespace::Conversation, capture.turn)
        else {
            bail!("fresh Writer user absent")
        };
        ensure!(
            user.conversation == root.id
                && capture.conversation == root.id
                && user.role == Role::User
                && user.mode == Mode::Writer
                && user.sequence == 0
                && user.id == crate::writer::stage(user.exchange, "user")
                && user.sources.contains(&capture.id),
            "fresh Writer capture/user differs"
        );
        let document = capture.before.document;
        ensure!(
            crate::memory::catalog::document_key(document)? == self.scope().key_sha256,
            "fresh Writer document differs"
        );
        let capture = (**capture).clone();
        drop(p);
        let original = Ledger::resident_images_from_evidence(capture, |m| {
            super::super::notebook::selected::read_selected_media(
                &mut snapshot,
                &ObjectRef {
                    sha256: m.sha256.clone(),
                    bytes: m.bytes,
                },
                reads,
            )
        })?
        .original_png;
        Ok(FreshFrame {
            capture: capture_ref.clone(),
            document,
            original,
        })
    }

    pub(crate) fn interpret_fresh_writer_locked(
        &self,
        store: &Store,
        accepted: &SelectionToken,
        prepared: &ResidentPreparedAcknowledgment,
        request: &SelectedReaderWrite,
        reads: &mut SelectedReadBudget,
        check: &mut dyn FnMut() -> Result<()>,
    ) -> Result<(
        SelectionToken,
        ResidentPreparedAcknowledgment,
        target::RecordRef,
    )> {
        ensure!(
            request.root == prepared.root && request.user == prepared.user,
            "fresh interpretation acknowledgment differs"
        );
        let SelectedReaderText::InterpretTarget { candidate } = &request.kind else {
            bail!("fresh interpretation variant")
        };
        check()?;
        let snapshot = self.writer_snapshot(store, Some(reads))?;
        ensure!(
            snapshot.token == *accepted,
            "fresh interpretation selection changed"
        );
        let p = SelectedDomainProjection::from_snapshot(&snapshot)?;
        let mut root = root(&p, &prepared.root)?.clone();
        let Record::Turn(user) = current(&p, &prepared.user)? else {
            bail!("fresh interpretation user variant")
        };
        ensure!(
            user.mode == Mode::Writer
                && user.role == Role::User
                && user.outcome == Outcome::Prepared
                && user.text.is_none()
                && user.sources == vec![prepared.capture.record_id]
                && user.sequence == 0
                && request.operation
                    == crate::writer::stage(user.exchange, "target-interpretation")
                && request.updated_ms >= root.updated_ms
                && request.updated_ms >= user.updated_ms,
            "fresh interpretation user/chronology differs"
        );
        let logical = user.exchange;
        let mut user = user.clone();
        user.outcome = Outcome::Interpreted;
        user.text = Some(request.text.clone());
        user.updated_ms = request.updated_ms;
        Ledger::validate_turn(&user)?;
        root.updated_ms = request.updated_ms;
        let mut records = vec![
            record(
                request.operation,
                store.actor_id,
                BTreeSet::from([prepared.root.revision_id]),
                Record::Root(root),
            )?,
            record(
                request.operation,
                store.actor_id,
                BTreeSet::from([prepared.user.revision_id]),
                Record::Turn(user.clone()),
            )?,
        ];
        records.extend(super::super::selected_reader_write::targets::interpret(
            &p,
            request,
            &user,
            store.actor_id,
            candidate,
        )?);
        let target = pending::pin(&records[2])?;
        let fingerprint = Ledger::fingerprint(&(
            "selected-fresh-target-v1",
            IntentSelectionEvidence::from_token(accepted),
            request,
            &prepared.capture,
        ))?;
        add_receipt(
            &mut records,
            request.operation,
            store.actor_id,
            &fingerprint,
            user.conversation,
            None,
        )?;
        let acknowledgment = ResidentPreparedAcknowledgment {
            root: pending::pin(&records[0])?,
            user: pending::pin(&records[1])?,
            capture: prepared.capture.clone(),
        };
        let mut merged = snapshot.selected_records;
        merged.extend_from_slice(&records);
        let mut refs = snapshot.transaction.selected.records;
        refs.extend(
            records
                .iter()
                .map(pending::pin)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .map(|p| p.object),
        );
        SelectedDomainProjection::from_mixed_records(&merged, &refs, accepted.scope())?
            .inspect(user.conversation)?;
        let publication = store.commit_selected_guarded(
            accepted,
            request.operation,
            records,
            BTreeMap::new(),
            reads,
            &[logical, request.operation],
            check,
        )?;
        ensure!(
            !publication.replayed,
            "fresh interpretation already historical"
        );
        Ok((
            publication.token,
            acknowledgment,
            target::RecordRef {
                namespace: target.namespace,
                record: target.record_id,
                revision: target.revision_id,
                object: target.object,
            },
        ))
    }
}
