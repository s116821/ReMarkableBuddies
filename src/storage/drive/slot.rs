//! Fixed-ID metadata creation only. Observations do not validate domain parentage,
//! select a local aggregate, qualify OAuth interoperability or authorize native work.
use super::*;

const MAX_DESCRIPTOR: u64 = 64 * 1024;
const MAX_REQUEST: usize = 8 * 1024;
const KIND: &str = "commit-slot";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotBinding {
    pub group: Uuid,
    pub scope_sha256: String,
    /// Already agreed by all contenders for the base. This API never allocates it.
    pub slot_id: String,
}
impl SlotBinding {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.group.is_nil() && valid_digest(&self.scope_sha256),
            "invalid slot binding"
        );
        id(&self.slot_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotPointer {
    pub binding: SlotBinding,
    pub descriptor_id: String,
    pub descriptor_sha256: String,
}
impl SlotPointer {
    pub(crate) fn validate(&self) -> Result<()> {
        self.properties().map(|_| ())
    }
    pub(super) fn properties(&self) -> Result<BTreeMap<String, String>> {
        self.binding.validate()?;
        id(&self.descriptor_id)?;
        ensure!(
            self.descriptor_id != self.binding.slot_id && valid_digest(&self.descriptor_sha256),
            "invalid slot descriptor"
        );
        let properties: BTreeMap<String, String> = BTreeMap::from([
            ("buddy_protocol".into(), "1".into()),
            ("kind".into(), KIND.into()),
            ("collection".into(), self.binding.group.to_string()),
            ("descriptor_id".into(), self.descriptor_id.clone()),
            ("sha256".into(), self.descriptor_sha256.clone()),
            ("scope_sha256".into(), self.binding.scope_sha256.clone()),
        ]);
        ensure!(
            properties.len() <= 30 && properties.iter().all(|(k, v)| k.len() + v.len() <= 124),
            "slot properties exceed provider bound"
        );
        Ok(properties)
    }
    fn from_metadata(binding: &SlotBinding, metadata: RemoteFile) -> Result<Self> {
        ensure!(
            metadata.id == binding.slot_id && !metadata.trashed,
            "slot identity unavailable"
        );
        let get = |key: &str| {
            metadata
                .app_properties
                .get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("incomplete slot pointer"))
        };
        let pointer = Self {
            binding: binding.clone(),
            descriptor_id: get("descriptor_id")?,
            descriptor_sha256: get("sha256")?,
        };
        ensure!(
            metadata.app_properties == pointer.properties()?,
            "slot binding or protocol mismatch"
        );
        Ok(pointer)
    }
}

/// Constructed only after checking account binding and descriptor bytes remotely.
/// No serialization or Debug of credential/payload state.
pub struct PreparedSlot {
    pointer: SlotPointer,
    reference: ObjectRef,
    application: String,
    account: String,
}
impl std::fmt::Debug for PreparedSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSlot")
            .field("pointer", &self.pointer)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerMatch {
    Same,
    Different,
}
pub struct SlotObservation {
    pub pointer: SlotPointer,
    /// Bounded, hash-verified opaque bytes. Domain owner must check schema/base/closure.
    pub descriptor: Vec<u8>,
    pub candidate_match: PointerMatch,
}

/// A read of an agreed location, without any local publication candidate.
/// Absent includes provider not-visible/404; it never grants create authority.
#[derive(Debug)]
pub enum AgreedSlotObservation {
    Absent,
    Invalid(SlotInvalid),
    Present(AgreedSlotValue),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotInvalid {
    Metadata,
    Descriptor,
}
impl std::fmt::Display for SlotInvalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid agreed slot data")
    }
}
impl std::error::Error for SlotInvalid {}

pub struct AgreedSlotValue {
    pub pointer: SlotPointer,
    pub descriptor: Vec<u8>,
}
impl std::fmt::Debug for AgreedSlotValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgreedSlotValue")
            .field("pointer", &self.pointer)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for SlotObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlotObservation")
            .field("pointer", &self.pointer)
            .field("candidate_match", &self.candidate_match)
            .finish_non_exhaustive()
    }
}

impl GoogleDrive {
    fn slot_descriptor(&mut self, pointer: &SlotPointer) -> Result<Vec<u8>> {
        pointer.properties()?;
        let metadata = self.metadata(&pointer.descriptor_id)?;
        let tag = metadata.tag().map_err(|_| SlotInvalid::Descriptor)?;
        if !(metadata.id == pointer.descriptor_id
            && !metadata.trashed
            && tag.collection == pointer.binding.group
            && tag.kind == "object"
            && tag.sha256 == pointer.descriptor_sha256)
        {
            return Err(SlotInvalid::Descriptor.into());
        }
        let bytes = self.download(&pointer.descriptor_id, MAX_DESCRIPTOR)?;
        if bytes.is_empty() || digest(&bytes) != pointer.descriptor_sha256 {
            return Err(SlotInvalid::Descriptor.into());
        }
        Ok(bytes)
    }

    /// GET-only. The caller must additionally validate root/base/domain closure.
    /// Transport/auth failures remain errors, distinct from invalid occupied data.
    pub fn observe_agreed_slot(&mut self, binding: &SlotBinding) -> Result<AgreedSlotObservation> {
        binding.validate()?;
        self.authorize(binding.group)?;
        self.read_agreed_slot(binding)
    }
    fn read_agreed_slot(&mut self, binding: &SlotBinding) -> Result<AgreedSlotObservation> {
        let metadata = match self.metadata(&binding.slot_id) {
            Ok(metadata) => metadata,
            Err(error) if missing(&error) => return Ok(AgreedSlotObservation::Absent),
            Err(error) if error.is::<InvalidRemoteData>() => {
                return Ok(AgreedSlotObservation::Invalid(SlotInvalid::Metadata));
            }
            Err(error) => return Err(error),
        };
        let pointer = match SlotPointer::from_metadata(binding, metadata) {
            Ok(pointer) => pointer,
            Err(_) => return Ok(AgreedSlotObservation::Invalid(SlotInvalid::Metadata)),
        };
        let descriptor = match self.slot_descriptor(&pointer) {
            Ok(bytes) => bytes,
            Err(error)
                if missing(&error)
                    || error.is::<InvalidRemoteData>()
                    || error.is::<SlotInvalid>() =>
            {
                return Ok(AgreedSlotObservation::Invalid(SlotInvalid::Descriptor));
            }
            Err(error) => return Err(error),
        };
        Ok(AgreedSlotObservation::Present(AgreedSlotValue {
            pointer,
            descriptor,
        }))
    }

    pub fn prepare_slot(
        &mut self,
        binding: SlotBinding,
        descriptor_id: String,
        reference: ObjectRef,
    ) -> Result<PreparedSlot> {
        let pointer = SlotPointer {
            binding,
            descriptor_id,
            descriptor_sha256: reference.sha256.clone(),
        };
        pointer.properties()?;
        ensure!(
            reference.bytes > 0 && reference.bytes <= MAX_DESCRIPTOR,
            "slot descriptor exceeds supported bound"
        );
        self.authorize(pointer.binding.group)?;
        let bytes = self.slot_descriptor(&pointer)?;
        ensure!(
            bytes.len() as u64 == reference.bytes,
            "slot descriptor size mismatch"
        );
        Ok(PreparedSlot {
            pointer,
            reference,
            application: self.credentials.client_id.clone(),
            account: self.credentials.account_permission_id.clone(),
        })
    }

    /// Read the original prepared slot without creating or republishing anything.
    /// Missing/invalid data is not freedom; this evidence grants no domain/native
    /// authority or permission to send at another slot after local replacement.
    pub fn observe_prepared_slot(&mut self, prepared: &PreparedSlot) -> Result<SlotObservation> {
        ensure!(
            prepared.application == self.credentials.client_id
                && prepared.account == self.credentials.account_permission_id,
            "prepared slot application/account mismatch"
        );
        prepared.pointer.properties()?;
        self.authorize(prepared.pointer.binding.group)?;
        self.observe_pointer(&prepared.pointer)
    }
    fn observe_pointer(&mut self, pointer: &SlotPointer) -> Result<SlotObservation> {
        let value = match self.read_agreed_slot(&pointer.binding)? {
            AgreedSlotObservation::Present(value) => value,
            AgreedSlotObservation::Absent => anyhow::bail!("agreed slot not visible"),
            AgreedSlotObservation::Invalid(reason) => return Err(reason.into()),
        };
        Ok(SlotObservation {
            candidate_match: if value.pointer == *pointer {
                PointerMatch::Same
            } else {
                PointerMatch::Different
            },
            pointer: value.pointer,
            descriptor: value.descriptor,
        })
    }

    /// One create attempt at exactly the prepared ID, then an exact-ID observation.
    /// On an uncertain outcome, errors never mean "slot is free". Retry/read the
    /// same pinned ID; this method does not allocate, overwrite, delete or rebase.
    pub fn create_or_read_slot(&mut self, prepared: &PreparedSlot) -> Result<SlotObservation> {
        self.create_or_read_slot_with(prepared, |_| Ok(()), |_| Ok(()))
    }

    pub(super) fn create_or_read_slot_with(
        &mut self,
        prepared: &PreparedSlot,
        before_send: impl FnOnce(usize) -> Result<()>,
        after_send: impl FnOnce(Option<u16>) -> Result<()>,
    ) -> Result<SlotObservation> {
        ensure!(
            prepared.application == self.credentials.client_id
                && prepared.account == self.credentials.account_permission_id,
            "prepared slot application/account mismatch"
        );
        let pointer = &prepared.pointer;
        let properties = pointer.properties()?;
        self.authorize(pointer.binding.group)?;
        // Recheck staging immediately before adjudication, even on replay.
        let bytes = self.slot_descriptor(pointer)?;
        ensure!(
            bytes.len() as u64 == prepared.reference.bytes,
            "slot descriptor size mismatch"
        );
        let request = slot_request(&pointer.binding.slot_id, properties)?;
        let token = self.token()?;
        before_send(request.len())?;
        let response = self.send_slot_request(&token, request)?;
        after_send(response.as_ref().map(|r| r.status().as_u16()))?;
        if let Some(mut response) = response {
            let status = response.status().as_u16();
            if status != 409 && !(500..600).contains(&status) {
                check(&mut response)?;
            }
        }
        // Body submission may have committed remotely even on transport error;
        // observation below is the only acknowledgement. Never log that error.
        self.observe_pointer(pointer)
    }

    pub(super) fn send_slot_request(
        &self,
        token: &str,
        request: Vec<u8>,
    ) -> Result<Option<ureq::http::Response<ureq::Body>>> {
        Ok(self
            .bounded(self.agent.post(format!("{}/files", self.api)))?
            .query("fields", "id")
            .header("Authorization", token)
            .header("Content-Type", "application/json")
            .send(request)
            .ok())
    }
}

pub(super) fn slot_request(id: &str, properties: BTreeMap<String, String>) -> Result<Vec<u8>> {
    let request = serde_json::to_vec(&json!({
        "id": id,
        "name": "buddy-commit-slot",
        "mimeType": "application/octet-stream",
        "parents": ["appDataFolder"],
        "appProperties": properties,
    }))?;
    ensure!(
        request.len() <= MAX_REQUEST,
        "slot request exceeds supported bound"
    );
    Ok(request)
}

fn missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<TransportFailure>()
        .is_some_and(|failure| failure.status == 404)
}
