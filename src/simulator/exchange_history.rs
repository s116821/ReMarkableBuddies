use super::SimulatorLedger;
use crate::{
    conversation::{
        BindingReceipt, CompletionEvidence, ExportAssociation, ImageUse, Ledger, Mode, Outcome,
        Role, SourceObservation, Turn,
    },
    device::interaction::Interaction,
    storage::{digest, Media, Namespace, Uuid},
    workflow::exchange_history::{
        self as history, Content, Edit, Effect, History, OutcomeStatus, PageBackend, PageOwner,
        PageState, PresentationReceipt,
    },
};
use anyhow::{ensure, Result};
use serde::Deserialize;
use std::io::Cursor;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Case {
    Repeat,
    Composer,
    Unrelated,
    EditedExchange,
    Header,
    Status,
    Seal,
    Departure,
    Session,
    ForeignPage,
    Replaced,
    Restore,
    StaleReceipt,
    Incomplete,
    Unknown,
    WrongOperation,
    ChangedReadback,
    Refused,
    Cancel,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    mode: String,
    composer: String,
    cases: Vec<Case>,
}

struct ModeledPage {
    state: PageState,
    receipt: Option<PresentationReceipt>,
    fault: Option<Case>,
    calls: usize,
}

impl PageBackend for ModeledPage {
    fn take_presentation(&mut self) -> Option<PresentationReceipt> {
        self.receipt.take()
    }
    fn observe(&mut self) -> Result<PageState> {
        Ok(self.state.clone())
    }
    fn apply(&mut self, edit: &Edit) -> Effect {
        if edit.before != self.state {
            return Effect::Refused;
        }
        self.calls += 1;
        if matches!(self.fault, Some(Case::Refused)) {
            return Effect::Refused;
        }
        self.state.content = edit.expected.clone();
        self.state.seal = format!("modeled-revision-{}", self.calls);
        let result = match self.fault {
            Some(Case::Unknown) => return Effect::Unknown,
            Some(Case::WrongOperation) => Uuid::new_v4(),
            _ => edit.operation,
        };
        let page = self.state.clone();
        if matches!(self.fault, Some(Case::ChangedReadback)) {
            self.state.content.unrelated.push('!');
        }
        Effect::Verified {
            operation: result,
            page: Box::new(page),
        }
    }
}

fn turn(conversation: Uuid, exchange: Uuid, role: Role, outcome: Outcome, text: &str) -> Turn {
    Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange,
        sequence: 0,
        role,
        mode: Mode::Reader,
        outcome,
        text: Some(text.into()),
        sources: vec![],
        correction_of: None,
        created_ms: 1,
        updated_ms: 1,
        completion: None,
    }
}

fn seed(ledger: &Ledger, exchanges: usize) -> Result<(Uuid, PageOwner, Uuid, Uuid, Vec<u8>)> {
    let conversation = Uuid::new_v4();
    let owner = PageOwner {
        document: Uuid::new_v4(),
        page: Uuid::new_v4(),
        visit: "modeled-visit".into(),
        session: "modeled-session".into(),
    };
    let source = SourceObservation {
        document: owner.document,
        page: Uuid::new_v4(),
        session: owner.session.clone(),
        visit: "modeled-source-visit".into(),
        content_revision: Some("source-revision".into()),
        order_revision: Some("source-order".into()),
        capability_revision: "modeled".into(),
        evidence_procedure: "simulator-observation".into(),
    };
    let observed = SourceObservation {
        page: owner.page,
        visit: owner.visit.clone(),
        ..source.clone()
    };
    ledger.create(conversation, Uuid::new_v4(), 0)?;
    let root = || ledger.expected(Namespace::Conversation, conversation);
    let binding = BindingReceipt {
        operation: Uuid::new_v4(),
        conversation,
        request_fingerprint: "fixture-request".into(),
        adapter_fingerprint: "fixture-adapter".into(),
        source: source.clone(),
        intended_target: Some(owner.page),
        observed_document: owner.document,
        observed_page: owner.page,
        expected_order: vec![source.page],
        observed_order: vec![source.page, owner.page],
        persisted_revision: "fixture-insertion".into(),
        observed_ms: 1,
        evidence_origin: "synthetic".into(),
        acquisition: Default::default(),
    };
    ledger.bind(binding.operation, root()?, Default::default(), binding)?;
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_luma8(2, 2).write_to(&mut png, image::ImageFormat::Png)?;
    let png = png.into_inner();
    let media = Media {
        sha256: digest(&png),
        bytes: png.len() as u64,
        media_type: "image/png".into(),
    };
    ledger.stage_image(&media, png.as_slice())?;
    let source_id = Uuid::new_v4();
    for index in 0..exchanges {
        let exchange = Uuid::new_v4();
        let mut user = turn(
            conversation,
            exchange,
            Role::User,
            Outcome::Interpreted,
            &format!("Question {index}?"),
        );
        let images = if index + 1 == exchanges {
            user.sources.push(source_id);
            vec![ImageUse {
                id: source_id,
                conversation,
                turn: user.id,
                image: media.clone(),
                observation: source.clone(),
                width: 2,
                height: 2,
                purpose: "overview".into(),
                ordinal: 0,
                parent: None,
                parent_dimensions: None,
                crop: None,
                transform: [1., 0., 0., 1., 0., 0.],
                target: None,
                captured_ms: 1,
            }]
        } else {
            vec![]
        };
        ledger.append(Uuid::new_v4(), root()?, user, images)?;
        let text = format!("Answer {index}.\nPreserve newlines.");
        let mut answer = turn(
            conversation,
            exchange,
            Role::Assistant,
            Outcome::Completed,
            &text,
        );
        answer.completion = Some(CompletionEvidence {
            native_operation: Uuid::new_v4(),
            exact_text: text,
            observation: observed.clone(),
            procedure: "modeled-readback".into(),
        });
        ledger.append(Uuid::new_v4(), root()?, answer, vec![])?;
    }
    let export = ExportAssociation {
        id: Uuid::new_v4(),
        conversation,
        backend: "modeled-external".into(),
        native_note: Some("existing-note".into()),
        source_scope: "fixture".into(),
        source_revision: "original".into(),
        operation: Uuid::new_v4(),
        outcome: Outcome::Completed,
        attempt: None,
    };
    let export_id = export.id;
    ledger.associate_export(export.operation, root()?, Default::default(), export)?;
    Ok((conversation, owner, source_id, export_id, png))
}

fn run_case(case: Case, composer: &str, exchanges: usize) -> Result<()> {
    use Case::*;
    let fixture = SimulatorLedger::new();
    let mut ledger = fixture.open()?;
    let (conversation, owner, source_id, export_id, image) = seed(&ledger, exchanges)?;
    if matches!(case, Incomplete) {
        ledger.append(
            Uuid::new_v4(),
            ledger.expected(Namespace::Conversation, conversation)?,
            turn(
                conversation,
                Uuid::new_v4(),
                Role::User,
                Outcome::Interpreted,
                "Pending question",
            ),
            vec![],
        )?;
    }
    let original_records = ledger.inspect(conversation, false)?;
    let original_export = ledger.export_association(conversation, export_id)?;
    let projected = history::project(&ledger, conversation, &owner, composer)?;
    let before = PageState {
        owner,
        seal: "modeled-revision-initial".into(),
        content: Content {
            header: "ReMarkableBuddies".into(),
            composer: composer.into(),
            status: "Ready".into(),
            unrelated: "User ink and annotation".into(),
            exchanges: projected.exchanges,
        },
    };
    let mut page = ModeledPage {
        state: before.clone(),
        calls: 0,
        fault: None,
        receipt: Some(PresentationReceipt {
            operation: Uuid::new_v4(),
            page: before.clone(),
            root_revision: projected.root,
            binding_revision: projected.binding,
        }),
    };
    let mut history = History::default();
    if matches!(case, StaleReceipt | Incomplete) {
        if matches!(case, StaleReceipt) {
            page.receipt.as_mut().unwrap().root_revision = Uuid::new_v4();
        }
        ensure!(
            history.arm(&ledger, conversation, &mut page).is_err() && page.calls == 0,
            "stale receipt armed"
        );
        return Ok(());
    }
    history.arm(&ledger, conversation, &mut page)?;

    if matches!(case, Repeat) {
        ensure!(
            before.content.exchanges[0].turns[0].text == format!("Question {}?", exchanges - 1),
            "newest exchange order changed"
        );
        let mut removed = before.content.clone();
        removed.exchanges.remove(0);
        for _ in 0..2 {
            ensure!(
                history.handle(Interaction::Undo, &ledger, &mut page) == OutcomeStatus::Applied
                    && page.state.content == removed,
                "owned undo changed unrelated content"
            );
            ensure!(
                history.handle(Interaction::Undo, &ledger, &mut page) == OutcomeStatus::NoChange,
                "repeated undo dispatched"
            );
            ensure!(
                history.handle(Interaction::Redo, &ledger, &mut page) == OutcomeStatus::Applied
                    && page.state.content == before.content,
                "redo did not restore exact content"
            );
            ensure!(
                history.handle(Interaction::Redo, &ledger, &mut page) == OutcomeStatus::NoChange,
                "repeated redo dispatched"
            );
        }
        ensure!(page.calls == 4, "effect count changed");
    } else {
        let (expected, calls) = match case {
            Composer => {
                page.state.content.composer.push('!');
                (OutcomeStatus::Refused, 0)
            }
            Unrelated => {
                page.state.content.unrelated.push('!');
                (OutcomeStatus::Refused, 0)
            }
            EditedExchange => {
                page.state.content.exchanges[1].turns[0].text.push('!');
                (OutcomeStatus::Refused, 0)
            }
            Header => {
                page.state.content.header.push('!');
                (OutcomeStatus::Refused, 0)
            }
            Status => {
                page.state.content.status.push('!');
                (OutcomeStatus::Refused, 0)
            }
            Seal => {
                page.state.seal.push('!');
                (OutcomeStatus::Refused, 0)
            }
            Departure => {
                page.state.owner.visit.push('!');
                (OutcomeStatus::Refused, 0)
            }
            Session => {
                page.state.owner.session.push('!');
                (OutcomeStatus::Refused, 0)
            }
            ForeignPage => {
                page.state.owner.page = Uuid::new_v4();
                (OutcomeStatus::Refused, 0)
            }
            Replaced => {
                let request = turn(
                    conversation,
                    Uuid::new_v4(),
                    Role::User,
                    Outcome::Interpreted,
                    "Synthetic newer aggregate",
                );
                ledger.append(
                    Uuid::new_v4(),
                    ledger.expected(Namespace::Conversation, conversation)?,
                    request,
                    vec![],
                )?;
                (OutcomeStatus::Refused, 0)
            }
            Restore => {
                drop(ledger);
                ledger = fixture.open()?;
                ensure!(
                    History::default()
                        .arm(&ledger, conversation, &mut page)
                        .is_err(),
                    "reopened history recreated a grant"
                );
                (OutcomeStatus::Refused, 0)
            }
            Cancel => {
                history.handle(Interaction::Invalidated, &ledger, &mut page);
                (OutcomeStatus::Refused, 0)
            }
            Unknown | WrongOperation | ChangedReadback => {
                page.fault = Some(case);
                (OutcomeStatus::ReconcileRequired, 1)
            }
            Refused => {
                page.fault = Some(case);
                (OutcomeStatus::Refused, 1)
            }
            Repeat | StaleReceipt | Incomplete => unreachable!(),
        };
        let durable_before = ledger.inspect(conversation, false)?;
        ensure!(
            history.handle(Interaction::Undo, &ledger, &mut page) == expected
                && page.calls == calls,
            "history fault was not fenced"
        );
        page.state = before.clone();
        ensure!(
            history.handle(Interaction::Redo, &ledger, &mut page) == OutcomeStatus::Refused
                && page.calls == calls,
            "old ownership revived"
        );
        ensure!(
            ledger.inspect(conversation, false)? == durable_before,
            "fault modified durable conversation"
        );
    }
    if !matches!(case, Replaced) {
        ensure!(
            ledger.inspect(conversation, false)? == original_records,
            "effect modified durable records"
        );
        ensure!(
            history::project(&ledger, conversation, &before.owner, composer)?.exchanges
                == before.content.exchanges,
            "effect modified durable turns"
        );
        ensure!(
            ledger
                .expected(Namespace::Conversation, conversation)?
                .0
                .contains(&projected.root),
            "effect modified durable root"
        );
    }
    ensure!(ledger.image(source_id)? == image, "source bytes changed");
    ensure!(
        ledger.export_association(conversation, export_id)? == original_export,
        "external export association changed"
    );
    Ok(())
}

pub(super) fn run(scenario: Scenario) -> Result<serde_json::Value> {
    ensure!(
        scenario.mode == "exchange-history"
            && !scenario.cases.is_empty()
            && scenario.cases.len() <= 32
            && scenario.composer.len() <= 4096,
        "invalid exchange history fixture"
    );
    for case in &scenario.cases {
        run_case(*case, &scenario.composer, 2)?;
    }
    run_case(Case::Repeat, &scenario.composer, 1)?;
    Ok(
        serde_json::json!({"mode":"exchange-history","cases":scenario.cases.len()+1,"native_effects":0}),
    )
}

#[test]
fn maintained_exchange_history_scenario() {
    let scenario = serde_json::from_str(include_str!(
        "../../docs/simulator/scenarios/owned-exchange-history.json"
    ))
    .unwrap();
    run(scenario).unwrap();
}
