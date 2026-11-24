use super::*;
use crate::conversation::Role;
use anyhow::Context;

#[path = "../../../../../../tests/fixtures/epub.rs"]
mod publication;

fn book(experiment: &str) -> Vec<u8> {
    use publication::{package, xhtml, CONTAINER};
    let spine = package(
        r#"<item id="e" href="experiment.xhtml" media-type="application/xhtml+xml"/><item id="i" href="intro.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="i"/><itemref idref="e"/>"#,
    );
    publication::book(
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
            source_targets: false,
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
    Ok(())
}
