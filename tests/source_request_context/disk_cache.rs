//! Real owned cache through declared receiving; no runtime, model or tablet.
use super::*;
use remarkable_reader_buddy::{
    conversation::resolution::SourceDirective, retrieval::disk::DiskCache,
};

fn receive(
    ledger: &Ledger,
    request: &Turn,
    origin: &Turn,
    source: &ImageUse,
    key: &IndexKey,
    provider: &dyn Provider,
) -> Result<FrozenRequestContext> {
    ledger.source_request_context_with_directive(
        request.conversation,
        request.id,
        None,
        SourceDirective::Explicit(association(origin, source, key)),
        budget(),
        Some(provider),
        counter,
    )
}

#[test]
fn reopened_cache_receives_followups_and_reloads_changed_material() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (first, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let material = document(book, "original");
    // The receiving key is independently built, not discovered from a cache slot.
    let expected = Index::build(material.clone()).unwrap().key().clone();
    let cache = DiskCache::open(ledger.store()).unwrap();
    assert_eq!(cache.install(material).unwrap(), expected);
    drop(cache);
    drop(ledger);

    let ledger = fixture.ledger();
    let cache = DiskCache::open(ledger.store()).unwrap();
    let slot = fixture
        .0
        .join(format!("cache/normalized-sources-v1/{book}.json"));
    let before = fs::read(&slot).unwrap();
    let initial = receive(&ledger, &first, &first, &source, &expected, &cache).unwrap();
    let frozen = initial.encoded_json().to_owned();
    assert_eq!(initial.request(), &first);
    assert_eq!(initial.retrieval().unwrap().passages[0].page, Some(1));
    assert_eq!(initial.retrieval().unwrap().source, expected);
    let followup = append(&ledger, user(conversation, "orbital"), vec![]);
    let next = receive(&ledger, &followup, &first, &source, &expected, &cache).unwrap();
    assert_eq!(next.request(), &followup);
    assert_eq!(next.history(), std::slice::from_ref(&first));
    assert_eq!(next.retrieval().unwrap().passages[0].page, Some(9));
    assert_eq!(next.counted_tokens(), next.encoded_json().len());
    assert_eq!(fs::read(&slot).unwrap(), before);

    let mut changed = document(book, "replacement");
    changed.units[1].text = "orbital replacement observation".into();
    let fresh = Index::build(changed.clone()).unwrap().key().clone();
    assert_eq!(cache.install(changed).unwrap(), fresh);
    assert_eq!(
        error(receive(
            &ledger, &followup, &first, &source, &expected, &cache
        )),
        ReceivingRefusal::RetrievalUnavailable
    );
    let replaced = receive(&ledger, &followup, &first, &source, &fresh, &cache).unwrap();
    assert_eq!(replaced.retrieval().unwrap().source, fresh);
    assert_eq!(
        replaced.retrieval().unwrap().passages[0].text,
        "orbital replacement observation"
    );
    assert_eq!(initial.encoded_json(), frozen);
}

#[test]
fn missing_and_corrupt_refuse_but_no_match_and_no_document_stay_distinct() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "unmatched"), book);
    let material = document(book, "original");
    let expected = Index::build(material.clone()).unwrap().key().clone();
    let cache = DiskCache::open(ledger.store()).unwrap();
    let provider: &dyn Provider = &cache;
    assert_eq!(
        error(receive(
            &ledger, &request, &request, &source, &expected, provider
        )),
        ReceivingRefusal::RetrievalUnavailable
    );
    cache.install(material).unwrap();
    let empty = receive(&ledger, &request, &request, &source, &expected, provider).unwrap();
    let result = empty.retrieval().unwrap();
    assert_eq!(result.source, expected);
    assert!(result.passages.is_empty());
    assert!(!result.budget_limited);

    let slot = fixture
        .0
        .join(format!("cache/normalized-sources-v1/{book}.json"));
    fs::write(&slot, b"invalid private content").unwrap();
    assert_eq!(
        error(receive(
            &ledger, &request, &request, &source, &expected, provider
        )),
        ReceivingRefusal::RetrievalUnavailable
    );
    let no_document = ledger
        .source_request_context_with_directive(
            conversation,
            request.id,
            None,
            SourceDirective::Explicit(RequestSource::NoDocument),
            budget(),
            Some(provider),
            counter,
        )
        .unwrap();
    assert!(no_document.retrieval().is_none());
    assert_eq!(fs::read(&slot).unwrap(), b"invalid private content");
    fs::remove_file(&slot).unwrap();
    assert_eq!(
        error(receive(
            &ledger, &request, &request, &source, &expected, provider
        )),
        ReceivingRefusal::RetrievalUnavailable
    );
}

#[test]
fn receiving_keeps_complete_frame_limits_with_cache_provider() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger();
    let conversation = create(&ledger);
    let book = Uuid::new_v4();
    let (request, source) = with_source(&ledger, user(conversation, "acceleration"), book);
    let cache = DiskCache::open(ledger.store()).unwrap();
    let mut material = document(book, "original");
    // Keep the complete result above receiving's 512-byte minimum reservation.
    material.units[0].text = format!("acceleration {}", "bounded context ".repeat(40));
    let expected = cache.install(material).unwrap();
    let complete = receive(&ledger, &request, &request, &source, &expected, &cache).unwrap();
    let size = complete.encoded_json().len();
    let run = |bytes, tokens| {
        let mut limits = budget();
        limits.max_encoded_bytes = bytes;
        limits.context.provider_token_limit = Some(tokens);
        ledger.source_request_context_with_directive(
            conversation,
            request.id,
            None,
            SourceDirective::Explicit(association(&request, &source, &expected)),
            limits,
            Some(&cache as &dyn Provider),
            counter,
        )
    };
    assert!(run(size, size).is_ok());
    // The reduced reservation reaches the real Index, which refuses a direct
    // passage that no longer fits; receiving preserves its sanitized refusal.
    assert_eq!(
        error(run(size - 1, size)),
        ReceivingRefusal::RetrievalUnavailable
    );
    assert_eq!(
        error(run(size, size - 1)),
        ReceivingRefusal::TokenLimit {
            tokens: size,
            limit: size - 1,
        }
    );
}
