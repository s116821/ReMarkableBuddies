//! Memory-only ownership of one complete Q&A. This module performs no device I/O.
use crate::device::native_text::NativeText;
use anyhow::{ensure, Context, Result};

/// Bound selection latency; longer answers still render but do not own history.
pub const MAX_CHARACTERS: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub document: String,
    pub page: String,
    /// A visit revision, not just the final page UUID.
    pub visit: String,
    /// Includes the native application's process start identity.
    pub session: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageState {
    pub owner: Owner,
    pub content: NativeText,
    /// Opaque backend seal of the complete persisted page, including unknown data.
    pub seal: Vec<u8>,
    /// Set only by a backend that has established the editing contract.
    pub supported: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Undo,
    Redo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Select from the verified insertion cursor; includes all typed newlines.
    DeleteSuffix { characters: usize },
    /// Undo only the previously verified, exclusively owned range deletion.
    RestoreDeletion,
    /// Redo that same owned range deletion.
    RepeatDeletion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Empty,
    Applied,
    Undone,
    Busy,
}

#[derive(Clone)]
struct Record {
    before: NativeText,
    applied: NativeText,
    removed: Option<NativeText>,
    observed: PageState,
    characters: usize,
    state: State,
}

#[derive(Default)]
pub struct History {
    record: Option<Record>,
    pending: Option<(Record, Command)>,
}

fn preserves_prior(before: &NativeText, after: &NativeText) -> bool {
    let Some(last) = before.paragraphs.last() else {
        return false;
    };
    if !last.characters.is_empty() {
        return false;
    }
    // The empty insertion paragraph becomes the owned Q line. Its old style is
    // not unrelated visible content; every preceding paragraph remains exact.
    let prefix = before.paragraphs.len() - 1;
    after.paragraphs.len() >= prefix
        && before.paragraphs[..prefix] == after.paragraphs[..prefix]
        && before.root_layout == after.root_layout
        && before.scene_records == after.scene_records
}

impl History {
    pub fn pending_text(&self) -> Option<String> {
        self.pending
            .as_ref()
            .map(|(record, command)| match command {
                Command::RestoreDeletion => record.applied.text(),
                _ => record.before.text(),
            })
    }
    pub fn state(&self) -> State {
        if self.pending.is_some() {
            State::Busy
        } else {
            self.record.as_ref().map_or(State::Empty, |r| r.state)
        }
    }

    /// Used for departure, new iteration, any unrelated edit/input, restart,
    /// incomplete observation and uncertain native outcomes. Never compensates.
    pub fn discard(&mut self) {
        self.record = None;
        self.pending = None;
    }

    pub fn arm(&mut self, before: PageState, applied: PageState, block: &str) -> bool {
        self.discard();
        if !before.supported
            || !applied.supported
            || before.owner != applied.owner
            || block.is_empty()
            || block.len() > MAX_CHARACTERS
            || !block.ends_with('\n')
            || !block.bytes().all(|c| c == b'\n' || (32..=126).contains(&c))
            || applied.content.text() != format!("{}{block}", before.content.text())
            || !preserves_prior(&before.content, &applied.content)
        {
            return false;
        }
        self.record = Some(Record {
            before: before.content,
            applied: applied.content.clone(),
            removed: None,
            observed: applied,
            characters: block.len(),
            state: State::Applied,
        });
        true
    }

    /// Consumes usable ownership before returning a mutation command. A failed
    /// or interrupted mutation can therefore never leave a usable stale record.
    pub fn begin(&mut self, action: Action, current: &PageState) -> Option<Command> {
        if self.pending.is_some() {
            self.discard();
            return None;
        }
        let record = self.record.take()?;
        if &record.observed != current || !current.supported {
            return None;
        }
        let command = match (record.state, action, record.removed.is_some()) {
            (State::Applied, Action::Undo, false) => Command::DeleteSuffix {
                characters: record.characters,
            },
            (State::Applied, Action::Undo, true) => Command::RepeatDeletion,
            (State::Undone, Action::Redo, true) => Command::RestoreDeletion,
            _ => {
                self.record = Some(record);
                return None;
            }
        };
        self.pending = Some((record, command));
        Some(command)
    }

    /// The backend must settle to the expected complete native state and report
    /// all intervening external events before calling this. Errors discard history.
    pub fn finish(&mut self, result: Result<PageState>) -> Result<()> {
        let (mut record, command) = self
            .pending
            .take()
            .context("Q&A history was invalidated during mutation")?;
        let observed = result?;
        ensure!(
            observed.supported && observed.owner == record.observed.owner,
            "Q&A page/session changed during mutation"
        );
        match command {
            Command::DeleteSuffix { .. } => {
                ensure!(
                    observed.content.text() == record.before.text()
                        && preserves_prior(&record.before, &observed.content),
                    "Native deletion did not preserve prior content"
                );
                record.removed = Some(observed.content.clone());
                record.state = State::Undone;
            }
            Command::RepeatDeletion => {
                ensure!(
                    record.removed.as_ref() == Some(&observed.content),
                    "Native history deletion did not match the owned entry"
                );
                record.state = State::Undone;
            }
            Command::RestoreDeletion => {
                ensure!(
                    observed.content == record.applied,
                    "Native history restore did not match the owned Q&A"
                );
                record.state = State::Applied;
            }
        }
        record.observed = observed;
        self.record = Some(record);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::native_text::{Character, Paragraph};
    fn page(text: &str, seal: u8) -> PageState {
        PageState {
            owner: Owner {
                document: "doc".into(),
                page: "page".into(),
                visit: "1:9".into(),
                session: "pid:start".into(),
            },
            content: NativeText {
                paragraphs: text
                    .split('\n')
                    .map(|line| Paragraph {
                        style: 1,
                        characters: line
                            .chars()
                            .map(|value| Character {
                                value,
                                bold: false,
                                italic: false,
                            })
                            .collect(),
                    })
                    .collect(),
                root_layout: vec![1],
                scene_records: vec![vec![2, 3]],
            },
            seal: vec![seal],
            supported: true,
        }
    }
    const BEFORE: &str = "Header\n\nQ: old?\nA: old.\n---\n";
    const BLOCK: &str = "Q: new?\n\nA: x^2 +/- 1.\n---\n";
    fn armed() -> (History, PageState, PageState) {
        let before = page(BEFORE, 1);
        let after = page(&format!("{BEFORE}{BLOCK}"), 2);
        let mut history = History::default();
        assert!(history.arm(before.clone(), after.clone(), BLOCK));
        (history, before, after)
    }
    #[test]
    fn repeated_toggles_use_only_the_verified_native_delete_entry() {
        let (mut history, mut removed, mut applied) = armed();
        assert_eq!(history.begin(Action::Redo, &applied), None);
        assert_eq!(
            history.begin(Action::Undo, &applied),
            Some(Command::DeleteSuffix {
                characters: BLOCK.len()
            })
        );
        assert_eq!(history.state(), State::Busy);
        removed.seal = vec![3];
        history.finish(Ok(removed.clone())).unwrap();
        assert_eq!(history.begin(Action::Undo, &removed), None);
        for generation in 4..10 {
            assert_eq!(
                history.begin(Action::Redo, &removed),
                Some(Command::RestoreDeletion)
            );
            applied.seal = vec![generation];
            history.finish(Ok(applied.clone())).unwrap();
            assert_eq!(
                history.begin(Action::Undo, &applied),
                Some(Command::RepeatDeletion)
            );
            removed.seal = vec![generation + 10];
            history.finish(Ok(removed.clone())).unwrap();
        }
        assert_eq!(history.state(), State::Undone);
    }
    #[test]
    fn departure_return_session_restart_and_unrelated_edits_discard_ownership() {
        for change in 0..5 {
            let (mut history, _, mut current) = armed();
            match change {
                0 => current.owner.visit = "1:11".into(),
                1 => current.owner.session = "newpid:start".into(),
                2 => current.seal.push(99),
                3 => current.content.scene_records.push(vec![4]),
                _ => current.content.paragraphs[0].style = 3,
            }
            assert_eq!(history.begin(Action::Undo, &current), None);
            assert_eq!(history.state(), State::Empty);
        }
        let (mut history, _, current) = armed();
        history.discard();
        assert_eq!(history.begin(Action::Undo, &current), None);
    }
    #[test]
    fn partial_output_unsettled_files_and_unsupported_input_never_arm() {
        let before = page(BEFORE, 1);
        let mut history = History::default();
        assert!(!history.arm(before.clone(), page(&format!("{BEFORE}Q: new"), 2), BLOCK));
        assert!(!history.arm(before.clone(), before.clone(), BLOCK));
        let unicode = "Q: π?\nA: π.\n---\n";
        assert!(!history.arm(
            before.clone(),
            page(&format!("{BEFORE}{unicode}"), 2),
            unicode
        ));
        let mut changed = page(&format!("{BEFORE}{BLOCK}"), 2);
        changed.content.paragraphs[0].style = 3;
        assert!(!history.arm(before, changed, BLOCK));
        assert_eq!(history.state(), State::Empty);
    }

    #[test]
    fn oversized_answers_never_start_an_unbounded_native_selection() {
        let before = page(BEFORE, 1);
        let limit = format!("{}\n", "a".repeat(MAX_CHARACTERS - 1));
        let mut history = History::default();
        assert!(history.arm(before.clone(), page(&format!("{BEFORE}{limit}"), 2), &limit));
        let oversized = format!("a{limit}");
        assert!(!history.arm(before, page(&format!("{BEFORE}{oversized}"), 3), &oversized));
        assert_eq!(history.state(), State::Empty);
    }
    #[test]
    fn failed_partial_or_interrupted_mutations_have_no_compensation_or_stale_record() {
        for failure in 0..4 {
            let (mut history, before, applied) = armed();
            history.begin(Action::Undo, &applied).unwrap();
            let result = match failure {
                0 => history.finish(Err(anyhow::anyhow!("input failed"))),
                1 => history.finish(Ok(applied.clone())),
                2 => {
                    history.discard();
                    history.finish(Ok(before))
                }
                _ => {
                    let mut changed = before;
                    changed.content.scene_records.push(vec![222, 1, 1, 42]);
                    changed.seal.push(99);
                    history.finish(Ok(changed))
                }
            };
            assert!(result.is_err());
            assert_eq!(history.state(), State::Empty);
            assert_eq!(history.begin(Action::Redo, &applied), None);
        }
    }

    #[test]
    fn observed_native_cycle_preserves_visible_ink_and_prior_paragraphs() {
        use crate::device::native_text::read;
        let files: [&[u8]; 4] = [
            include_bytes!("../../tests/fixtures/native-history/ink-before.rm"),
            include_bytes!("../../tests/fixtures/native-history/ink-applied.rm"),
            include_bytes!("../../tests/fixtures/native-history/ink-deleted.rm"),
            include_bytes!("../../tests/fixtures/native-history/ink-restored.rm"),
        ];
        let states: Vec<_> = files
            .iter()
            .enumerate()
            .map(|(index, bytes)| {
                let mut state = page("", index as u8);
                state.content = read(bytes).unwrap();
                state
            })
            .collect();
        let mut history = History::default();
        assert!(history.arm(
            states[0].clone(),
            states[1].clone(),
            "Q: REM15?\n\nA: a+b=c; x^2 +/- 1.\n---\n"
        ));
        assert_eq!(
            history.begin(Action::Undo, &states[1]),
            Some(Command::DeleteSuffix { characters: 36 })
        );
        history.finish(Ok(states[2].clone())).unwrap();
        assert_eq!(
            history.begin(Action::Redo, &states[2]),
            Some(Command::RestoreDeletion)
        );
        history.finish(Ok(states[3].clone())).unwrap();
        assert_eq!(history.state(), State::Applied);
        assert_eq!(
            states[0].content.paragraphs[..23],
            states[2].content.paragraphs[..23]
        );
        assert!(states
            .iter()
            .all(|s| s.content.scene_records == states[0].content.scene_records));
    }
}
