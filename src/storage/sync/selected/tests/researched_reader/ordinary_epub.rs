use super::*;
use crate::{conversation::Role, epub_fixture};
use anyhow::Context;

fn book(experiment: &str) -> Vec<u8> {
    use epub_fixture::{package, xhtml, CONTAINER};
    let spine = package(
        r#"<item id="e" href="experiment.xhtml" media-type="application/xhtml+xml"/><item id="i" href="intro.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="i"/><itemref idref="e"/>"#,
    );
    epub_fixture::book(
        CONTAINER,
        &spine,
        &[
            ("book/experiment.xhtml", xhtml(experiment)),
            (
                "book/intro.xhtml",
                xhtml("<p>The introduction describes an unrelated diagram.</p>"),
            ),
        ],
    )
}

pub(super) fn check_open_book(executable: &std::path::Path) -> Result<()> {
    let fixture = Fixture::new();
    fs::DirBuilder::new().mode(0o700).create(&fixture.0)?;
    let document = Uuid::new_v4();
    let page = Uuid::new_v4();
    let original = fixture.0.join(format!("{document}.epub"));
    let first = book("<p>The experiment measures alpha acceleration.</p>");
    let revised = book("<p>The revised experiment measures beta velocity.</p>");
    fs::write(&original, &first)?;
    // The tablet's rendered pagination is independent of the two spine units.
    files::atomic_json(
        &fixture.0.join(format!("{document}.content")),
        &json!({
            "fileType":"epub", "cPages":{"original":{"value":600},"pages":[
                {"id":page,"idx":{"value":"a"},"redir":{"value":310}}
            ]}
        }),
    )?;
    fs::write(
        fixture.0.join(format!("{document}.pdf")),
        pdf_bytes("DECOY rendered PDF"),
    )?;
    let questions = [
        (OPT_OUT, "book/experiment.xhtml", "alpha acceleration"),
        (
            "Describe the introduction diagram. Don't use the internet.",
            "book/intro.xhtml",
            "unrelated diagram",
        ),
        (
            "What measures beta velocity? Don't use the internet.",
            "book/experiment.xhtml",
            "beta velocity",
        ),
    ];
    let mut change_book = |round: usize, _: &std::path::Path| {
        if round == 2 {
            files::atomic(&original, &revised)?;
        }
        Ok(())
    };
    let scope = researched_fixture::Scope::new(None, false);
    let mut model = Scripted::new(Reading::Deny, false, false, Events::default());
    model.ordinary_pdf_questions = questions.iter().map(|&(question, _, _)| question).collect();
    let (results, records) = crate::workflow::ordinary_pdf_receiver_fixture(
        document,
        page,
        fixture.0.clone(),
        executable.into(),
        crate::workflow::OrdinaryPdfFixtureRounds {
            epub: true,
            rendered: None,
            source_targets: false,
            edit_book_during_navigation: false,
            count: questions.len(),
            before_round: Some(&mut change_book),
        },
        |_, _, _| model,
        |_, _| Ok(()),
    )?;
    assert_eq!(results.len(), questions.len());
    assert_eq!(
        scope.constructions(),
        0,
        "the questions explicitly declined web research"
    );
    assert_eq!(
        scope
            .ordinary_pdf_workers()
            .iter()
            .map(|(_, reused)| *reused)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
    for (round, (result, (question, section, excerpt))) in
        results.into_iter().zip(questions).enumerate()
    {
        result.with_context(|| format!("Answer EPUB question {}", round + 1))?;
        let user = records
            .iter()
            .find_map(|record| match record {
                Record::Turn(turn)
                    if turn.role == Role::User && turn.text.as_deref() == Some(question) =>
                {
                    Some(turn)
                }
                _ => None,
            })
            .context("Verified EPUB question absent")?;
        let evidence = records
            .iter()
            .find_map(|record| match record {
                Record::RequestEvidence(evidence) if evidence.request_turn == user.id => {
                    Some(evidence)
                }
                _ => None,
            })
            .context("EPUB retrieval evidence absent")?;
        let frame: Value = serde_json::from_str(&evidence.frame_json)?;
        let retrieval = &frame["retrieval"];
        assert_eq!(frame["request"]["text"], question);
        assert_eq!(retrieval["source"]["document"], json!(document));
        assert_eq!(retrieval["source"]["format"], "epub");
        assert_eq!(
            retrieval["source"]["source_sha256"],
            digest(if round == 2 { &revised } else { &first })
        );
        let cited = &retrieval["passages"][0];
        assert!(cited["page"].is_null());
        assert_eq!(cited["section"], section);
        assert!(cited["text"]
            .as_str()
            .is_some_and(|text| text.contains(excerpt)));
        let mut answers = records.iter().filter_map(|record| match record {
            Record::Turn(turn)
                if turn.role == Role::Assistant && turn.exchange == user.exchange =>
            {
                Some(turn)
            }
            _ => None,
        });
        let answer = answers.next().context("Saved EPUB answer absent")?;
        assert!(
            answers.next().is_none(),
            "Multiple answers to the same EPUB question"
        );
        assert!(answer
            .text
            .as_deref()
            .is_some_and(|text| text.contains(&format!("[S1] Document {document}, {section}"))));
    }
    let rendered = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let question = questions[2].0;
    let mut model = Scripted::new(Reading::Deny, false, false, Events::default());
    model.ordinary_pdf_questions = vec![question];
    let (results, _) = crate::workflow::ordinary_pdf_receiver_fixture(
        document,
        page,
        fixture.0.clone(),
        executable.into(),
        crate::workflow::OrdinaryPdfFixtureRounds {
            epub: true,
            rendered: Some(rendered.clone()),
            source_targets: false,
            edit_book_during_navigation: false,
            count: 1,
            before_round: None,
        },
        |_, _, _| model,
        |_, _| Ok(()),
    )?;
    for result in results {
        result.context("Place the EPUB answer after its original page")?;
    }
    let rendered = rendered.borrow();
    let [shown] = rendered.as_slice() else {
        anyhow::bail!("Expected exactly one rendered EPUB answer")
    };
    assert!(shown.contains(question));
    assert!(shown.contains(&format!("[S1] Document {document}, book/experiment.xhtml")));
    drop(scope);
    check_marked_book_without_search(executable, &fixture.0, document, page)?;
    check_bound_failure_after_book_edit(executable, &fixture.0, document, page)
}

fn check_bound_failure_after_book_edit(
    executable: &std::path::Path,
    library: &std::path::Path,
    document: Uuid,
    page: Uuid,
) -> Result<()> {
    use crate::conversation::AttemptReason;

    let question = "What measures beta velocity? Don't use the internet.";
    let rendered = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let scope = researched_fixture::Scope::new(None, false);
    let mut model = Scripted::new(Reading::Deny, false, false, Events::default());
    model.ordinary_pdf_questions = vec![question];
    model.answer_refused = true;
    let frames = model.frames.clone();
    let (mut results, records) = crate::workflow::ordinary_pdf_receiver_fixture(
        document,
        page,
        library.into(),
        executable.into(),
        crate::workflow::OrdinaryPdfFixtureRounds {
            epub: true,
            rendered: Some(rendered.clone()),
            source_targets: false,
            edit_book_during_navigation: true,
            count: 1,
            before_round: None,
        },
        |_, _, _| model,
        |_, _| Ok(()),
    )?;
    let error = results
        .pop()
        .context("Bound EPUB result absent")?
        .expect_err("Expected the provider refusal");
    assert!(error.is::<crate::llm::ProviderRefused>(), "{error:#}");
    assert!(
        rendered.borrow().is_empty(),
        "The edited source reached the renderer"
    );
    assert_eq!(scope.constructions(), 0);
    assert_eq!(scope.ordinary_pdf_workers().len(), 1);
    assert_eq!(frames.lock().unwrap().len(), 3);
    let failure = records
        .iter()
        .find_map(|record| match record {
            Record::OutcomeFact(fact) if fact.reason == AttemptReason::ProviderFailure => {
                Some(fact)
            }
            _ => None,
        })
        .context("The original provider failure was not retained")?;
    assert!(records.iter().any(|record| matches!(record,
        Record::Turn(turn) if turn.id == failure.turn && turn.text.as_deref() == Some(question)
    )));
    assert!(!records.iter().any(|record| matches!(record,
        Record::Turn(turn) if turn.role == Role::Assistant
    )));
    Ok(())
}

fn check_marked_book_without_search(
    executable: &std::path::Path,
    library: &std::path::Path,
    document: Uuid,
    page: Uuid,
) -> Result<()> {
    use crate::conversation::{receiving::memory::research::ResearchRefusal, Outcome};

    let question = "Beta velocity? which measure";
    let rendered = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let scope = researched_fixture::Scope::new(None, false);
    let mut model = Scripted::new(Reading::Clear, true, false, Events::default());
    model.ordinary_pdf_questions = vec![question];
    model.mark_ordinary_pdf_question = true;
    let frames = model.frames.clone();
    let (mut results, records) = crate::workflow::ordinary_pdf_receiver_fixture(
        document,
        page,
        library.into(),
        executable.into(),
        crate::workflow::OrdinaryPdfFixtureRounds {
            epub: true,
            rendered: Some(rendered.clone()),
            source_targets: true,
            edit_book_during_navigation: false,
            count: 1,
            before_round: None,
        },
        |ledger, _, _| {
            // Remove only the synthetic fixture credential. The real search
            // selector must report unavailability before a transport is built.
            fs::remove_file(ledger.store().paths.credentials.join("fixture_model_key"))
                .expect("remove the synthetic web credential");
            model
        },
        |ledger, records| {
            let conversation = records
                .iter()
                .find_map(|record| match record {
                    Record::Turn(turn) if turn.role == Role::User => Some(turn.conversation),
                    _ => None,
                })
                .context("Saved marked EPUB question absent")?;
            let validated = ledger.inspect(conversation, false)?;
            ensure!(
                validated.iter().any(|record| matches!(record,
                    Record::Receipt(receipt) if receipt.marked_question.is_some()
                )),
                "Marked EPUB receipt absent"
            );
            Ok(())
        },
    )?;
    let error = results
        .pop()
        .context("Marked EPUB result absent")?
        .expect_err("Required web research unexpectedly answered");
    assert!(error.is::<ResearchRefusal>(), "{error:#}");
    assert_eq!(scope.constructions(), 0);
    assert_eq!(scope.ordinary_pdf_workers().len(), 1);
    assert_eq!(frames.lock().unwrap().len(), 2);
    assert_eq!(*rendered.borrow(), [format!("Q: {question}\n")]);
    assert!(records.iter().any(|record| matches!(record,
        Record::Turn(turn) if turn.role == Role::User && turn.outcome == Outcome::Failed
            && turn.text.as_deref() == Some(question)
    )));
    assert!(!records.iter().any(|record| matches!(record,
        Record::Turn(turn) if turn.role == Role::Assistant
    )));
    let evidence = records
        .iter()
        .find_map(|record| match record {
            Record::RequestEvidence(evidence) => Some(evidence),
            _ => None,
        })
        .context("Marked EPUB retrieval evidence absent")?;
    let frame: Value = serde_json::from_str(&evidence.frame_json)?;
    assert_eq!(frame["retrieval"]["source"]["format"], "epub");
    assert_eq!(frame["retrieval"]["source"]["document"], json!(document));
    Ok(())
}
