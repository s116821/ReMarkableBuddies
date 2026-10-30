use super::*;

#[test]
fn ranked_receiving_keeps_request_history_source_and_prior_frozen_bytes() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (first, source) = with_source(
        &ledger,
        user(conversation, "  capacitance dielectric?\n"),
        book,
    );
    let mut material = document(book, "authored-ranking-source");
    material.units = [
        format!(
            "capacitance dielectric magnetic inductance {}",
            "background ".repeat(100)
        ),
        "capacitance dielectric stores charge".into(),
        "magnetic inductance stores energy".into(),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, text)| SourceUnit {
        page: Some(i as u32 + 1),
        section: None,
        text,
    })
    .collect();
    let index = Index::build(material).unwrap();
    let mut limits = budget();
    limits.max_passages = 1;
    let initial = ledger
        .source_request_context(
            conversation,
            first.id,
            None,
            association(&first, &source, index.key()),
            limits.clone(),
            Some(&index),
            counter,
        )
        .unwrap();
    assert_eq!(initial.retrieval().unwrap().passages[0].page, Some(2));
    let frozen = initial.encoded_json().to_owned();
    let followup = append(&ledger, user(conversation, "magnetic inductance"), vec![]);
    let before = snapshot(&fixture.0);
    let received = ledger
        .source_request_context(
            conversation,
            followup.id,
            None,
            association(&first, &source, index.key()),
            limits,
            Some(&index),
            counter,
        )
        .unwrap();
    assert_eq!(received.request(), &followup);
    assert_eq!(received.history(), [first]);
    assert_eq!(received.retrieval().unwrap().passages[0].page, Some(3));
    assert_eq!(&received.retrieval().unwrap().source, index.key());
    assert_eq!(initial.encoded_json(), frozen);
    assert_eq!(snapshot(&fixture.0), before);
    assert_eq!(received.counted_tokens(), received.encoded_json().len());
}
