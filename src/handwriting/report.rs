//! Historical data reports with owned command lifetime, never execution authority.
use super::*;
use std::io::Write;

pub const MAX_REPORT: usize = 256 * 1024 + 1;
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Command {
    Confirm,
    Update,
    Inspect,
    Query,
    Forget,
    Status,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Complete,
    CompleteEmpty,
    Partial,
    Busy,
    Unavailable,
    Unsupported,
    InvalidInput,
    Conflict,
    Failed,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MutationOutcome {
    NotRequested,
    NotDispatched,
    Observed,
    Unknown,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FactState {
    Value,
    Tombstone,
    Unsupported,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Availability {
    Available,
    Unavailable,
    NotExamined,
    NotApplicable,
}
#[derive(Serialize)]
pub struct Fact {
    pub record: Uuid,
    pub revision: Uuid,
    pub parents: BTreeSet<Uuid>,
    pub head: Option<bool>,
    pub conflicted: Option<bool>,
    pub state: FactState,
    pub value: Option<Correction>,
    pub sample_availability: Availability,
}
#[derive(Serialize)]
pub struct Diagnostics {
    pub scanned: u64,
    pub matched: u64,
    pub omitted: u64,
    pub conflicted: u64,
    pub unsupported: u64,
    pub missing_media: u64,
    pub mutation_outcome: MutationOutcome,
}
#[derive(Serialize)]
pub struct Report {
    pub handwriting_version: u32,
    pub command: Option<Command>,
    pub status: Status,
    pub failure: Option<Failure>,
    pub operation: Option<Uuid>,
    pub transaction: Option<Uuid>,
    pub record: Option<Uuid>,
    pub historical: bool,
    pub facts: Vec<Fact>,
    pub diagnostics: Diagnostics,
    #[serde(skip)]
    pub(super) _guard: Option<Box<dyn Send>>,
}
impl Request {
    pub fn command(&self) -> Command {
        match self {
            Self::Confirm(_) => Command::Confirm,
            Self::Update(_) => Command::Update,
            Self::Inspect(_) => Command::Inspect,
            Self::Query(_) => Command::Query,
            Self::Forget(_) => Command::Forget,
            Self::Status(_) => Command::Status,
        }
    }
    pub fn profile(&self) -> Uuid {
        match self {
            Self::Confirm(v) | Self::Update(v) => v.profile,
            Self::Inspect(v) => v.profile,
            Self::Query(v) => v.profile,
            Self::Forget(v) => v.profile,
            Self::Status(v) => v.profile,
        }
    }
    pub fn operation(&self) -> Option<Uuid> {
        match self {
            Self::Confirm(v) | Self::Update(v) => Some(v.operation),
            Self::Forget(v) => Some(v.operation),
            Self::Status(v) => Some(v.operation),
            _ => None,
        }
    }
    pub fn record(&self) -> Option<Uuid> {
        let correction = match self {
            Self::Confirm(v) | Self::Update(v) => v.correction,
            Self::Inspect(v) => v.correction,
            Self::Forget(v) => v.correction,
            Self::Status(v) => v.correction,
            Self::Query(_) => return None,
        };
        record_id(self.profile(), correction).ok()
    }
}
impl Report {
    pub fn new(request: Option<&Request>) -> Self {
        let command = request.map(Request::command);
        Self {
            handwriting_version: 1,
            command,
            status: Status::CompleteEmpty,
            failure: None,
            operation: request.and_then(Request::operation),
            transaction: None,
            record: request.and_then(Request::record),
            historical: false,
            facts: vec![],
            diagnostics: Diagnostics {
                scanned: 0,
                matched: 0,
                omitted: 0,
                conflicted: 0,
                unsupported: 0,
                missing_media: 0,
                mutation_outcome: match command {
                    Some(Command::Confirm | Command::Update | Command::Forget) => {
                        MutationOutcome::NotDispatched
                    }
                    Some(Command::Status) => MutationOutcome::Unknown,
                    _ => MutationOutcome::NotRequested,
                },
            },
            _guard: None,
        }
    }
    pub(super) fn fail(&mut self, failure: Failure) {
        self.failure = Some(failure);
        self.facts.clear();
        self.status = match failure {
            Failure::UnsupportedMode | Failure::UnsupportedRecord | Failure::InvalidVersion => {
                Status::Unsupported
            }
            Failure::OwnershipBusy => Status::Busy,
            Failure::HeadsConflict | Failure::OperationConflict | Failure::ImmutableChange => {
                Status::Conflict
            }
            Failure::StoreUnavailable | Failure::MediaUnavailable | Failure::OutputLimit => {
                Status::Unavailable
            }
            Failure::CommitUnknown | Failure::StorageFailure => Status::Failed,
            _ => Status::InvalidInput,
        };
    }
    pub fn write_to(&self, output: &mut impl Write) -> std::io::Result<()> {
        let mut bytes =
            serde_json::to_vec(self).map_err(|_| std::io::Error::other("output-failure"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_REPORT {
            return Err(std::io::Error::other("output-failure"));
        }
        output.write_all(&bytes)
    }
    pub(super) fn size(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |v| v.len() + 1)
    }
}
