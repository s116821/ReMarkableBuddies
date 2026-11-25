//! Scoped observations for the mandatory composed receiver tests only.
use crate::conversation::SelectedReaderWrite;
use crate::storage::selection::SelectionToken;
use anyhow::{ensure, Context, Result};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
pub(crate) struct GenerationWitness {
    pub(crate) accepted: SelectionToken,
    pub(crate) request: SelectedReaderWrite,
}

struct State {
    endpoint: Option<String>,
    constructions: usize,
    lose_generated_ack: bool,
    generation: Option<GenerationWitness>,
    #[cfg(feature = "installed-resident-output")]
    replace_interpreted_pdf_turn: bool,
    #[cfg(feature = "installed-resident-output")]
    ordinary_pdf_workers: Vec<(crate::storage::Uuid, bool)>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
}

pub(crate) struct Scope(Rc<RefCell<State>>);
impl Scope {
    pub(crate) fn new(endpoint: Option<String>, lose_generated_ack: bool) -> Self {
        let state = Rc::new(RefCell::new(State {
            endpoint,
            constructions: 0,
            lose_generated_ack,
            generation: None,
            #[cfg(feature = "installed-resident-output")]
            replace_interpreted_pdf_turn: false,
            #[cfg(feature = "installed-resident-output")]
            ordinary_pdf_workers: Vec::new(),
        }));
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            assert!(active.is_none(), "nested researched Reader fixture");
            *active = Some(state.clone());
        });
        Self(state)
    }

    pub(crate) fn constructions(&self) -> usize {
        self.0.borrow().constructions
    }

    pub(crate) fn generation(&self) -> Option<GenerationWitness> {
        self.0.borrow().generation.clone()
    }

    #[cfg(feature = "installed-resident-output")]
    pub(crate) fn replace_interpreted_pdf_turn(&self) {
        self.0.borrow_mut().replace_interpreted_pdf_turn = true;
    }

    #[cfg(feature = "installed-resident-output")]
    pub(crate) fn ordinary_pdf_workers(&self) -> Vec<(crate::storage::Uuid, bool)> {
        self.0.borrow().ordinary_pdf_workers.clone()
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.borrow_mut().take();
        });
    }
}

#[cfg(feature = "installed-resident-output")]
pub(in crate::workflow) fn record_ordinary_pdf_worker(
    index: &crate::retrieval::pdf_worker::PendingIndex,
) {
    ACTIVE.with(|active| {
        if let Some(state) = active.borrow().as_ref() {
            state
                .borrow_mut()
                .ordinary_pdf_workers
                .push(index.fixture_request());
        }
    });
}

/// Change only the interpreted turn's storage revision after the real commit.
#[cfg(feature = "installed-resident-output")]
pub(in crate::workflow) fn after_ordinary_pdf_interpret(
    ledger: &crate::conversation::Ledger,
    user: &crate::conversation::Turn,
) -> Result<()> {
    use crate::storage::{Namespace, Uuid};

    let replace = ACTIVE.with(|active| {
        active.borrow().as_ref().is_some_and(|state| {
            std::mem::take(&mut state.borrow_mut().replace_interpreted_pdf_turn)
        })
    });
    if replace {
        let store = ledger.store();
        let original = store.conditional_snapshot(Namespace::Conversation, user.conversation)?;
        let mut successor = store
            .value(Namespace::Conversation, user.id)?
            .context("Fixture interpreted PDF turn absent")?;
        successor.parents = [successor.revision_id].into();
        successor.revision_id = Uuid::new_v4();
        successor.operation_id = Uuid::new_v4();
        store.commit(vec![successor], Default::default())?;
        ensure!(
            store.conditional_snapshot(Namespace::Conversation, user.conversation)? == original,
            "Fixture replacement changed the conversation root or Store generation"
        );
    }
    Ok(())
}

/// Called only after the real destination/model admission and selected stage
/// decision. Fixture input cannot grant either of those permissions.
pub(in crate::workflow) fn endpoint(selected: &str) -> Result<String> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some(state) = active.as_ref() else {
            return Ok(selected.to_owned());
        };
        let mut state = state.borrow_mut();
        state.constructions += 1;
        let endpoint = state
            .endpoint
            .as_ref()
            .context("fixture search endpoint absent")?;
        let url = reqwest::Url::parse(endpoint)?;
        ensure!(
            url.scheme() == "http"
                && url
                    .host_str()
                    .and_then(|host| host
                        .trim_matches(&['[', ']'][..])
                        .parse::<std::net::IpAddr>()
                        .ok())
                    .is_some_and(|ip| ip.is_loopback())
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/v1/responses",
            "fixture search endpoint must be an exact loopback destination"
        );
        Ok(endpoint.clone())
    })
}

pub(crate) fn record_generation(
    accepted: &SelectionToken,
    request: &SelectedReaderWrite,
) -> Result<()> {
    ACTIVE.with(|active| {
        if let Some(state) = active.borrow().as_ref() {
            let mut state = state.borrow_mut();
            ensure!(
                state.generation.is_none(),
                "fixture Generated input already observed"
            );
            state.generation = Some(GenerationWitness {
                accepted: accepted.clone(),
                request: request.clone(),
            });
        }
        Ok(())
    })
}

/// The actual publication has already returned Published. Simulate losing that
/// acknowledgment before Attempt can install it, without changing the receipt.
pub(crate) fn after_generated_ack() -> Result<()> {
    ACTIVE.with(|active| {
        if let Some(state) = active.borrow().as_ref() {
            ensure!(
                !state.borrow().lose_generated_ack,
                "synthetic Generated acknowledgment loss"
            );
        }
        Ok(())
    })
}
