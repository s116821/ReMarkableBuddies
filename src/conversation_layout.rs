//! Read-only logical presentation. A plan never grants current native authority.
use crate::conversation::{ContextView, Role, Turn};
use crate::conversation_feedback::FeedbackDescription;
use crate::storage::{Uuid, MAX_ITEMS};
use anyhow::{ensure, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationIdentity {
    /// Already validated historical binding supplied by the domain owner.
    Persisted {
        conversation: Uuid,
        document: Uuid,
        page: Uuid,
    },
    Unbound,
    Legacy,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub header_end: f32,
    pub input_end: f32,
    pub status_end: f32,
}
impl Default for Geometry {
    fn default() -> Self {
        Self {
            header_end: 0.08,
            input_end: 0.20,
            status_end: 0.25,
        }
    }
}
#[derive(Debug)]
pub struct Exchange<'a> {
    pub id: Uuid,
    /// Original chronological order within the exchange, never a new ledger.
    pub turns: Vec<&'a Turn>,
}
#[derive(Debug)]
pub struct PagePlan<'a> {
    pub conversation: Uuid,
    pub document: Uuid,
    pub page: Uuid,
    pub title: &'static str,
    pub draft: &'a str,
    pub header: Region,
    pub input: Region,
    pub status: Region,
    pub history: Region,
    /// Newest exchange first. Full original text and source links stay borrowed.
    pub exchanges: Vec<Exchange<'a>>,
}
/// One explicit historical description linked to an original exchange and turn.
/// The page retains original borrowed content; this is not an inserted error
/// turn, live status, presentation acknowledgment or current native authority.
#[derive(Debug)]
pub struct HistoricalFeedbackPlan<'a> {
    pub page: PagePlan<'a>,
    pub exchange: Uuid,
    pub description: FeedbackDescription,
}
/// Project an already validated domain context. No pixel/OCR/cache/device inputs,
/// I/O, model calls, draft changes or native operations exist in this interface.
/// The caller retains responsibility for About interpretation and current binding
/// admission. Full history is logical content; native overflow/scroll/focus/edit
/// capability and transactional region rendering remain integration gates.
pub fn plan<'a>(
    identity: PresentationIdentity,
    context: &'a ContextView,
    draft: &'a str,
    geometry: Geometry,
) -> Result<PagePlan<'a>> {
    let PresentationIdentity::Persisted {
        conversation,
        document,
        page,
    } = identity
    else {
        anyhow::bail!("presentation identity unavailable");
    };
    ensure!(
        !conversation.is_nil() && !document.is_nil() && !page.is_nil(),
        "nil presentation identity"
    );
    let Geometry {
        header_end,
        input_end,
        status_end,
    } = geometry;
    ensure!(
        header_end.is_finite()
            && input_end.is_finite()
            && status_end.is_finite()
            && 0.0 < header_end
            && header_end < input_end
            && input_end < status_end
            && status_end < 1.0,
        "invalid normalized regions"
    );
    ensure!(
        context.turns.len() <= MAX_ITEMS,
        "presentation turn bound exceeded"
    );
    let mut ids = BTreeSet::new();
    let mut previous = None;
    let mut exchanges: BTreeMap<Uuid, Vec<&Turn>> = BTreeMap::new();
    for turn in &context.turns {
        ensure!(
            turn.conversation == conversation && !turn.id.is_nil() && !turn.exchange.is_nil(),
            "foreign or nil turn identity"
        );
        ensure!(
            ids.insert(turn.id) && previous.is_none_or(|sequence| sequence < turn.sequence),
            "duplicate or nonchronological turn"
        );
        ensure!(
            turn.sources.iter().all(|id| !id.is_nil())
                && turn.sources.iter().copied().collect::<BTreeSet<_>>().len()
                    == turn.sources.len(),
            "invalid original source references"
        );
        previous = Some(turn.sequence);
        let exchange = exchanges.entry(turn.exchange).or_default();
        ensure!(
            !exchange.is_empty() || turn.role == Role::User,
            "exchange starts without original user"
        );
        exchange.push(turn);
    }
    let mut exchanges: Vec<_> = exchanges
        .into_iter()
        .map(|(id, turns)| Exchange { id, turns })
        .collect();
    exchanges.sort_by_key(|exchange| {
        std::cmp::Reverse(exchange.turns.last().expect("nonempty exchange").sequence)
    });
    let region = |start, end| Region {
        x: 0.0,
        y: start,
        width: 1.0,
        height: end - start,
    };
    Ok(PagePlan {
        conversation,
        document,
        page,
        title: "ReMarkableBuddies",
        draft,
        header: region(0.0, header_end),
        input: region(header_end, input_end),
        status: region(input_end, status_end),
        history: region(status_end, 1.0),
        exchanges,
    })
}
