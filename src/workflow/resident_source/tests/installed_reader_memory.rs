use super::*;
use crate::memory::{self, Provider as _};

const QUESTION: &str = "Which compass should I use?";
const NOTEBOOK: &str = "The brass compass is for this notebook.";
const OTHER: &str = "The plastic compass is for the other notebook.";
const GLOBAL: &str = "Check the compass before departure.";

#[derive(Default)]
struct ReaderModel {
    parts: Vec<String>,
    calls: Rc<RefCell<Vec<Vec<String>>>>,
}

impl crate::LLMEngine for ReaderModel {
    fn add_text_content(&mut self, text: &str) {
        self.parts.push(text.to_owned());
    }

    fn add_image_content(&mut self, _image: &str) {}

    fn clear_content(&mut self) {
        self.parts.clear();
    }

    fn execute(&mut self) -> Result<String> {
        let mut calls = self.calls.borrow_mut();
        calls.push(self.parts.clone());
        match calls.len() {
            1 => Ok(format!("QUESTION: {QUESTION}\nQUESTION_BOX: 130,35,200,50\nSELECTION_CENTER: 385,230\nSELECTION_SUBJECT: compass\n---\nANSWER: A compass.")),
            2 => Ok(format!("TRANSCRIPTION: {QUESTION}")),
            3 => Ok(format!("ANSWER: {NOTEBOOK}")),
            _ => anyhow::bail!("Unexpected Reader request"),
        }
    }
}

#[test]
fn installed_reader_uses_only_its_notebook_and_global_memory_after_transcription() -> Result<()> {
    use crate::conversation::Role;
    let mode = "reconnect-source-handoff";
    let (mut workflow, store, base, events, _notify) =
        workflow(mode, vec![released(Instant::now())], Duration::ZERO);
    let document = ConversationId::new_v4();
    let owner = workflow.resident_source.as_mut().unwrap();
    owner.selection = InstalledReaderSelection::fixture_with_installed_output();
    owner.factory = Box::new(OwnedFactory {
        source: Some((document, ConversationId::new_v4())),
        events,
        mode,
        delay: Duration::ZERO,
    });

    let profile = ConversationId::new_v4();
    let memory = Arc::new(memory::LocalMemory::new_local(store.clone(), profile)?);
    for (scope, claim, value) in [
        (
            memory::Scope::Document { document },
            "compass choice",
            NOTEBOOK,
        ),
        (
            memory::Scope::Document {
                document: ConversationId::new_v4(),
            },
            "compass choice",
            OTHER,
        ),
        (memory::Scope::Global, "compass check", GLOBAL),
    ] {
        memory.put(memory::Mutation {
            operation: ConversationId::new_v4(),
            expected: Default::default(),
            fact: memory::Fact {
                key: memory::Key {
                    profile,
                    scope,
                    fact: ConversationId::new_v4(),
                },
                claim: claim.into(),
                subject: "compass".into(),
                value: value.into(),
                tags: vec![],
                provenance: memory::Provenance {
                    basis: memory::Basis::UserConfirmed,
                    description: "Synthetic notebook choice".into(),
                    conversation: None,
                    turn: None,
                    source: None,
                },
                confidence_basis_points: 1000,
                observed_ms: 1,
                expires_ms: None,
            },
        })?;
    }
    let model = ReaderModel::default();
    let calls = model.calls.clone();
    let mut reader = Orchestrator::new(workflow, model, Ledger::new(store.clone()));
    reader.set_subject_memory(Some(memory));
    reader.run_iteration()?;
    let calls = calls.borrow();
    let [analysis, transcription, answer] = calls.as_slice() else {
        anyhow::bail!("Expected analysis, transcription and an enriched answer");
    };
    for parts in [analysis, transcription] {
        assert!(parts.iter().all(|part| ![NOTEBOOK, OTHER, GLOBAL]
            .iter()
            .any(|fact| part.contains(*fact))));
    }
    let mut snapshots = answer
        .iter()
        .filter_map(|part| part.strip_prefix("ASSOCIATIVE_MEMORY: "));
    let snapshot: memory::ContextData = serde_json::from_str(
        snapshots
            .next()
            .expect("expected an associative-memory snapshot"),
    )?;
    assert!(snapshots.next().is_none());
    let mut values: Vec<_> = snapshot
        .facts
        .iter()
        .map(|item| item.fact.value.as_str())
        .collect();
    values.sort_unstable();
    assert_eq!(values, [GLOBAL, NOTEBOOK]);

    let ledger = Ledger::new(store.clone());
    let captures = store.snapshot_heads_matching(&[Namespace::Source], 2, |_| true)?;
    let [capture] = captures.as_slice() else {
        anyhow::bail!("Expected the source capture")
    };
    let Record::ResidentCapture(capture) = Ledger::decode(capture)? else {
        anyhow::bail!("Expected resident evidence")
    };
    assert_eq!(capture.correspondence.expected.document, document);
    let turns = ledger.inspect(capture.conversation, false)?;
    for (role, text) in [(Role::User, QUESTION), (Role::Assistant, NOTEBOOK)] {
        assert!(turns.iter().any(|record| matches!(record, Record::Turn(t) if t.role == role && t.text.as_deref() == Some(text))));
    }
    drop((ledger, reader, store));
    std::fs::remove_dir_all(base)?;
    Ok(())
}
