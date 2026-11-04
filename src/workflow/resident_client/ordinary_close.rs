//! One distinct durable development close attempt. Never an insertion journal.
use super::*;

const FILE: &str = "native-development-close-v1.json";
const LIMIT: u64 = 64 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u8,
    actor: Uuid,
    conversation: Uuid,
    request: String,
    sha256: String,
    known_not_dispatched: bool,
    response: Option<String>,
}
impl Record {
    fn validate(&self, actor: Uuid) -> Result<()> {
        ensure!(
            self.schema == 1
                && self.actor == actor
                && !actor.is_nil()
                && !self.conversation.is_nil(),
            "close journal ownership/schema mismatch"
        );
        let request = decode(&self.request)?;
        ensure!(
            matches!(
                request.frame().message,
                Message::Request(Command::CloseOrdinary(_))
            ) && crate::storage::digest(request.encoded()) == self.sha256,
            "invalid close journal request"
        );
        if let Some(response) = &self.response {
            ensure!(
                !self.known_not_dispatched,
                "close journal has conflicting outcomes"
            );
            let response = decode(response)?;
            let a = request.frame();
            let b = response.frame();
            ensure!(
                request.wire_revision() == response.wire_revision()
                    && a.scope == b.scope
                    && a.sequence == b.sequence
                    && a.operation == b.operation,
                "close reply correlation mismatch"
            );
            let Message::Response {
                request_sha256,
                reply,
            } = &b.message
            else {
                bail!("invalid close reply frame")
            };
            ensure!(
                *request_sha256 == request.fingerprint()
                    && matches!(
                        reply,
                        Reply::OrdinaryCloseDispatched | Reply::Rejected(_) | Reply::Indeterminate
                    ),
                "invalid close reply kind/digest"
            );
        }
        Ok(())
    }
}
pub(crate) fn require_unused(store: &Store) -> Result<()> {
    store.with_owned_local_io(|root, actor| {
        let path = root.join(FILE);
        files::safe_path(&path)?;
        if path.try_exists()? {
            let record: Record = files::json(&path, LIMIT)?;
            record.validate(actor)?;
            bail!("development close already consumed; no replay or new runtime")
        }
        Ok(())
    })
}
pub(super) fn reserve(
    store: &Store,
    conversation: Uuid,
    request: &Frame,
    expected: [u8; 32],
) -> Result<()> {
    ensure!(
        request.fingerprint().ok() == Some(expected),
        "close callback digest mismatch"
    );
    store.with_owned_local_io(|root, actor| {
        let path = root.join(FILE);
        ensure!(!path.try_exists()?, "close operation already consumed");
        let record = Record {
            schema: 1,
            actor,
            conversation,
            request: encode(request)?,
            sha256: digest(request)?,
            known_not_dispatched: false,
            response: None,
        };
        record.validate(actor)?;
        files::create_identity(&path, &record)
    })
}
pub(super) fn finish(
    store: &Store,
    request: &Frame,
    response: Option<&Frame>,
    known_not_dispatched: bool,
) -> Result<()> {
    store.with_owned_local_io(|root, actor| {
        let path = root.join(FILE);
        let mut record: Record = files::json(&path, LIMIT)?;
        record.validate(actor)?;
        ensure!(
            record.request == encode(request)?
                && record.response.is_none()
                && !record.known_not_dispatched,
            "close journal changed before outcome"
        );
        record.response = response.map(encode).transpose()?;
        record.known_not_dispatched = known_not_dispatched;
        record.validate(actor)?;
        let bytes = serde_json::to_vec(&record)?;
        ensure!(bytes.len() as u64 <= LIMIT, "close journal exceeds bound");
        files::atomic(&path, &bytes)
    })
}

pub(crate) fn require_dispatched(store: &Store) -> Result<()> {
    store.with_owned_local_io(|root, actor| {
        let record: Record = files::json(&root.join(FILE), LIMIT)?;
        record.validate(actor)?;
        let response = decode(
            record
                .response
                .as_ref()
                .context("close dispatch reply not retained")?,
        )?;
        ensure!(
            matches!(
                response.frame().message,
                Message::Response {
                    reply: Reply::OrdinaryCloseDispatched,
                    ..
                }
            ),
            "fixture reopen requires positive close dispatch"
        );
        Ok(())
    })
}
