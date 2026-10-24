use remarkable_reader_buddy::conversation::{ContextView, Mode, Outcome, Role, Turn};
use remarkable_reader_buddy::conversation_layout::{plan, Geometry, PresentationIdentity};
use remarkable_reader_buddy::storage::Uuid;
fn context(turns: Vec<Turn>) -> ContextView {
    ContextView {
        turns,
        sources: vec![],
        captures: vec![],
        legacy_captures: vec![],
        development_captures: vec![],
        missing_media: vec![],
        selection: None,
    }
}
fn identity(conversation: Uuid) -> PresentationIdentity {
    PresentationIdentity::Persisted {
        conversation,
        document: Uuid::new_v4(),
        page: Uuid::new_v4(),
    }
}
fn turn(
    conversation: Uuid,
    exchange: Uuid,
    sequence: u64,
    role: Role,
    mode: Mode,
    text: &str,
) -> Turn {
    Turn {
        id: Uuid::new_v4(),
        conversation,
        exchange,
        sequence,
        role: role.clone(),
        mode,
        outcome: if role == Role::User {
            Outcome::Interpreted
        } else {
            Outcome::Generated
        },
        text: Some(text.into()),
        sources: vec![Uuid::new_v4()],
        correction_of: None,
        created_ms: 100,
        updated_ms: 100,
        completion: None,
    }
}
#[test]
fn mixed_exchanges_prepend_without_mutating_chronological_context_or_draft() {
    let c = Uuid::new_v4();
    let old = Uuid::new_v4();
    let new = Uuid::new_v4();
    let ctx = context(vec![
        turn(c, old, 1, Role::User, Mode::Reader, "question"),
        turn(c, old, 2, Role::Assistant, Mode::Reader, "answer"),
        turn(c, new, 3, Role::User, Mode::Writer, "refine"),
        turn(c, new, 4, Role::Assistant, Mode::Writer, "refinement"),
    ]);
    let original = serde_json::to_vec(&ctx.turns).unwrap();
    let draft = "my edited draft\nkeep it";
    let p = plan(identity(c), &ctx, draft, Geometry::default()).unwrap();
    assert_eq!(
        p.exchanges.iter().map(|e| e.id).collect::<Vec<_>>(),
        [new, old]
    );
    for exchange in &p.exchanges {
        assert_eq!(exchange.turns[0].role, Role::User);
        assert_eq!(exchange.turns[1].role, Role::Assistant);
    }
    assert_eq!(p.exchanges[0].turns[0].sources, ctx.turns[2].sources);
    assert_eq!(p.draft, draft);
    assert_eq!(serde_json::to_vec(&ctx.turns).unwrap(), original);
    assert!(std::ptr::eq(p.exchanges[0].turns[0], &ctx.turns[2]));
    assert!(p.header.y + p.header.height <= p.input.y);
    assert!(p.input.y + p.input.height <= p.status.y);
    assert!(p.status.y + p.status.height <= p.history.y);
    assert_eq!(p.history.y + p.history.height, 1.0);
}
#[test]
fn blank_start_has_input_and_title_without_fabricated_turn() {
    let ctx = context(vec![]);
    let p = plan(identity(Uuid::new_v4()), &ctx, "", Geometry::default()).unwrap();
    assert_eq!(p.title, "ReMarkableBuddies");
    assert_eq!(p.draft, "");
    assert!(p.exchanges.is_empty());
}
#[test]
fn long_answer_and_pending_interpretation_remain_exact_logical_content() {
    let c = Uuid::new_v4();
    let e = Uuid::new_v4();
    let text = "長い answer\n".repeat(10000);
    let ctx = context(vec![
        turn(c, e, 1, Role::User, Mode::Reader, "interpreted immediately"),
        turn(c, e, 2, Role::Assistant, Mode::Reader, &text),
    ]);
    let p = plan(identity(c), &ctx, "draft", Geometry::default()).unwrap();
    assert_eq!(
        p.exchanges[0].turns[0].text.as_deref(),
        Some("interpreted immediately")
    );
    assert_eq!(p.exchanges[0].turns[1].text.as_deref(), Some(text.as_str()));
    assert_eq!(p.exchanges[0].turns[1].outcome, Outcome::Generated);
}
#[test]
fn identity_is_independent_of_scrolled_header_typography_and_cache() {
    let c = Uuid::new_v4();
    let ctx = context(vec![]);
    let id = identity(c);
    // Visual state deliberately never enters the API: different fonts/scale,
    // invisible header and a foreign global cache cannot establish identity.
    for _visual_case in [
        "large serif",
        "small sans",
        "scrolled header absent",
        "foreign cache",
    ] {
        let p = plan(id, &ctx, "draft", Geometry::default()).unwrap();
        assert_eq!(p.conversation, c);
        assert_eq!(p.draft, "draft");
    }
    assert!(plan(
        PresentationIdentity::Unbound,
        &ctx,
        "draft",
        Geometry::default()
    )
    .is_err());
    assert!(plan(
        PresentationIdentity::Legacy,
        &ctx,
        "draft",
        Geometry::default()
    )
    .is_err());
    assert!(plan(identity(Uuid::nil()), &ctx, "draft", Geometry::default()).is_err());
}
#[test]
fn inconsistent_context_and_unknown_geometry_refuse_without_changes() {
    let c = Uuid::new_v4();
    let e = Uuid::new_v4();
    let good = turn(c, e, 1, Role::User, Mode::Reader, "original");
    for turns in [
        vec![turn(
            Uuid::new_v4(),
            e,
            1,
            Role::User,
            Mode::Reader,
            "foreign",
        )],
        vec![good.clone(), good.clone()],
        vec![
            turn(c, e, 2, Role::User, Mode::Reader, "later"),
            good.clone(),
        ],
        vec![turn(c, e, 1, Role::Assistant, Mode::Reader, "orphan")],
    ] {
        let ctx = context(turns);
        let before = serde_json::to_vec(&ctx.turns).unwrap();
        assert!(plan(identity(c), &ctx, "keep", Geometry::default()).is_err());
        assert_eq!(serde_json::to_vec(&ctx.turns).unwrap(), before);
    }
    let ctx = context(vec![good]);
    for header_end in [f32::NAN, f32::INFINITY, -0.1, 0.25] {
        assert!(plan(
            identity(c),
            &ctx,
            "keep",
            Geometry {
                header_end,
                ..Geometry::default()
            }
        )
        .is_err());
    }
}

#[test]
fn nil_duplicate_source_links_and_oversized_context_refuse() {
    let c = Uuid::new_v4();
    let e = Uuid::new_v4();
    let good = turn(c, e, 1, Role::User, Mode::Reader, "original");
    for sources in [vec![Uuid::nil()], vec![good.sources[0], good.sources[0]]] {
        let mut bad = good.clone();
        bad.sources = sources;
        let ctx = context(vec![bad]);
        assert!(plan(identity(c), &ctx, "draft", Geometry::default()).is_err());
    }
    let ctx = context(vec![good; remarkable_reader_buddy::storage::MAX_ITEMS + 1]);
    assert!(plan(identity(c), &ctx, "draft", Geometry::default()).is_err());
}
