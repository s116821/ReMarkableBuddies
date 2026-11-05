//! One distinct durable development fixture reopen attempt. Never an insertion journal.
use super::*;

const FILE: &str = "native-development-reopen-v1.json";
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
            "fixture reopen journal ownership/schema mismatch"
        );
        let request = decode(&self.request)?;
        ensure!(
            matches!(
                request.frame().message,
                Message::Request(Command::ReopenFixture(_))
            ) && crate::storage::digest(request.encoded()) == self.sha256,
            "invalid fixture reopen journal request"
        );
        if let Some(response) = &self.response {
            ensure!(
                !self.known_not_dispatched,
                "fixture reopen journal has conflicting outcomes"
            );
            let response = decode(response)?;
            let a = request.frame();
            let b = response.frame();
            ensure!(
                request.wire_revision() == response.wire_revision()
                    && a.scope == b.scope
                    && a.sequence == b.sequence
                    && a.operation == b.operation,
                "fixture reopen reply correlation mismatch"
            );
            let Message::Response {
                request_sha256,
                reply,
            } = &b.message
            else {
                bail!("invalid fixture reopen reply frame")
            };
            ensure!(
                *request_sha256 == request.fingerprint()
                    && matches!(
                        reply,
                        Reply::FixtureReopenDispatched | Reply::Rejected(_) | Reply::Indeterminate
                    ),
                "invalid fixture reopen reply kind/digest"
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
            bail!("development fixture reopen already consumed; no replay or new runtime")
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
        "fixture reopen callback digest mismatch"
    );
    store.with_owned_local_io(|root, actor| {
        let path = root.join(FILE);
        ensure!(
            !path.try_exists()?,
            "fixture reopen operation already consumed"
        );
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
            "fixture reopen journal changed before outcome"
        );
        record.response = response.map(encode).transpose()?;
        record.known_not_dispatched = known_not_dispatched;
        record.validate(actor)?;
        let bytes = serde_json::to_vec(&record)?;
        ensure!(
            bytes.len() as u64 <= LIMIT,
            "fixture reopen journal exceeds bound"
        );
        files::atomic(&path, &bytes)
    })
}
