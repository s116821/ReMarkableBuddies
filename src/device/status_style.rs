//! Bounded toolbar style lease. UI observations, not successful input writes,
//! establish transitions. An unresolved recovery record is never overwritten.
use anyhow::{ensure, Context, Result};
use image::GrayImage;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::Path,
};

pub type Preferences = BTreeMap<String, String>;
pub(crate) const COLORS: [&str; 9] = [
    "Black", "Gray", "White", "Blue", "Red", "Green", "Yellow", "Cyan", "Magenta",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub document: String,
    pub page: String,
    pub visit: String,
    pub session: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{imageops::replace, Luma};

    fn fixture(bytes: &[u8]) -> GrayImage {
        image::load_from_memory(bytes).unwrap().to_luma8()
    }
    fn closed() -> GrayImage {
        fixture(include_bytes!(
            "../../tests/fixtures/status-style/closed-fineliner.png"
        ))
    }
    fn prefs() -> Preferences {
        [
            ("LastActiveTool", "secondary"),
            ("LastPen", "Highlighterv2"),
            ("LastFinelinerv2Color", "Red"),
            ("LastFinelinerv2Size", "3"),
            ("LastHighlighterv2Color", "HighlighterYellow"),
            ("SecondaryPen", "Ballpointv2"),
            ("SecondaryBallpointv2Size", "2"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }
    fn identity() -> Identity {
        Identity {
            document: "e7f661f1-db6f-4dfc-854a-b38aff7f75de".into(),
            page: "f39ae285-3e0c-43dd-b27c-866dff7a24cd".into(),
            visit: "1:90".into(),
            session: "12:300".into(),
        }
    }
    fn box_at(image: &mut GrayImage, center: (u32, u32)) {
        for y in center.1 - 29..center.1 + 30 {
            for x in center.0 - 29..center.0 + 30 {
                image.put_pixel(x, y, Luma([0]));
            }
        }
    }
    struct Model {
        prefs: Preferences,
        persisted: Preferences,
        checkpoints: Vec<Recovery>,
        fail_checkpoint: Option<usize>,
        fail_before: bool,
        menu: bool,
        count: usize,
        fail: Option<usize>,
        identity: Identity,
    }
    impl Model {
        fn new() -> Self {
            let mut persisted = prefs();
            for (k, v) in [
                ("LastActiveTool", "primary"),
                ("LastPen", "Finelinerv2"),
                ("LastFinelinerv2Color", "Black"),
                ("LastFinelinerv2Size", "2"),
            ] {
                persisted.insert(k.into(), v.into());
            }
            Self {
                prefs: prefs(),
                persisted,
                checkpoints: Vec::new(),
                fail_checkpoint: None,
                fail_before: false,
                menu: false,
                count: 0,
                fail: None,
                identity: identity(),
            }
        }
    }
    impl StyleIo for Model {
        fn checkpoint(&mut self, record: &Recovery) -> Result<()> {
            ensure!(
                self.fail_checkpoint != Some(record.sequence),
                "Injected journal failure"
            );
            let initial = Recovery::new(
                self.identity.clone(),
                self.persisted.clone(),
                record.original_slot.clone(),
            )?;
            record.follows(self.checkpoints.last().unwrap_or(&initial))?;
            self.checkpoints.push(record.clone());
            Ok(())
        }
        fn observe(&mut self) -> Result<Observation> {
            let mut image = closed();
            replace(
                &mut image,
                &GrayImage::from_pixel(61, 123, Luma([255])),
                0,
                61,
            );
            box_at(
                &mut image,
                if self.prefs["LastActiveTool"] == "primary" {
                    (30, 91)
                } else {
                    (30, 153)
                },
            );
            if self.menu {
                for x in 61..280 {
                    image.put_pixel(x, 61, Luma([0]));
                }
                for y in 61..651 {
                    image.put_pixel(279, y, Luma([0]));
                }
                let fine = self.prefs["LastPen"] == "Finelinerv2";
                box_at(&mut image, GRID[if fine { 1 } else { 2 }]);
                if fine {
                    for x in 61..280 {
                        image.put_pixel(x, 430, Luma([0]));
                    }
                    if let Ok((color, width)) = fine_style(&self.prefs) {
                        box_at(&mut image, PALETTE[color]);
                        box_at(&mut image, WIDTHS[width]);
                    }
                }
            }
            Ok(Observation {
                identity: self.identity.clone(),
                preferences: self.persisted.clone(),
                image,
            })
        }
        fn press(&mut self, p: (u32, u32)) -> Result<()> {
            self.count += 1;
            assert_eq!(
                self.checkpoints.len(),
                self.count,
                "Input must have a durable intent checkpoint"
            );
            ensure!(
                !(self.fail_before && self.fail == Some(self.count)),
                "Input failed before effect"
            );
            match p {
                (30, 90) if self.prefs["LastActiveTool"] != "primary" => {
                    self.prefs.insert("LastActiveTool".into(), "primary".into());
                }
                (30, 90) => self.menu = !self.menu,
                (30, 150) => {
                    self.prefs
                        .insert("LastActiveTool".into(), "secondary".into());
                }
                p if p == GRID[1] => {
                    self.prefs.insert("LastPen".into(), "Finelinerv2".into());
                }
                p if p == GRID[2] => {
                    self.prefs.insert("LastPen".into(), "Highlighterv2".into());
                }
                p if PALETTE.contains(&p) => {
                    self.prefs.insert(
                        "LastFinelinerv2Color".into(),
                        COLORS[PALETTE.iter().position(|v| *v == p).unwrap()].into(),
                    );
                }
                p if WIDTHS.contains(&p) => {
                    self.prefs.insert(
                        "LastFinelinerv2Size".into(),
                        (WIDTHS.iter().position(|v| *v == p).unwrap() + 1).to_string(),
                    );
                }
                _ => anyhow::bail!("Unexpected modeled action"),
            }
            ensure!(self.fail != Some(self.count), "Partial input failure");
            Ok(())
        }
    }

    #[test]
    fn native_fixture_controls_and_refusal() {
        assert!(controls(&fixture(include_bytes!(
            "../../tests/fixtures/status-style/open-secondary-highlighter.png"
        )))
        .is_none());
        assert_eq!(
            controls(&fixture(include_bytes!(
                "../../tests/fixtures/status-style/closed-secondary-highlighter.png"
            )))
            .unwrap()
            .slot,
            "secondary"
        );
        for (bytes, slot) in [
            (
                include_bytes!("../../tests/fixtures/status-style/closed-highlighter.png")
                    .as_slice(),
                "primary",
            ),
            (
                include_bytes!("../../tests/fixtures/status-style/closed-secondary.png").as_slice(),
                "secondary",
            ),
        ] {
            let ui = controls(&fixture(bytes)).expect("Known closed native pen layout");
            assert_eq!(ui.slot, slot);
            assert!(!ui.menu);
        }
        assert_eq!(
            controls(&closed()).unwrap(),
            Controls {
                slot: "primary",
                menu: false,
                grid: None,
                fine: None
            }
        );
        let image = fixture(include_bytes!(
            "../../tests/fixtures/status-style/open-fineliner.png"
        ));
        assert_eq!(controls(&image).unwrap().fine, Some((0, 1)));
        let mut io = Model::new();
        io.menu = true;
        assert!(Lease::prepare(io.observe().unwrap()).unwrap().is_none());
        let mut state = io.observe().unwrap();
        state.image = GrayImage::from_pixel(768, 1024, Luma([255]));
        assert!(Lease::prepare(state).unwrap().is_none());
        assert_eq!(io.count, 0);
    }

    #[test]
    fn highlighter_secondary_and_nondefault_fine_restore_exactly() {
        let mut io = Model::new();
        let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
        lease.acquire(&mut io).unwrap();
        assert_eq!(fine_style(&io.prefs).unwrap(), (0, 1));
        assert_eq!(io.prefs["LastPen"], "Finelinerv2");
        assert!(!io.menu);
        // An asynchronous native save can change advisory values mid-lease;
        // it must not replace the earlier actual Red/Thick UI snapshot.
        io.persisted = io.prefs.clone();
        let count = io.count;
        // Holding/rechecking the lease performs no toolbar input.
        for _ in 0..20 {
            lease.observe(&mut io).unwrap();
        }
        assert_eq!(count, io.count);
        lease.restore(&mut io).unwrap();
        assert_eq!(io.prefs, prefs());
        assert!(!io.menu);
    }

    #[test]
    fn every_partial_acquisition_action_rolls_back() {
        for slot in ["primary", "secondary"] {
            for before in [false, true] {
                for failure in 1..=if slot == "primary" { 5 } else { 6 } {
                    let mut io = Model::new();
                    io.prefs.insert("LastActiveTool".into(), slot.into());
                    let original = io.prefs.clone();
                    io.fail = Some(failure);
                    io.fail_before = before;
                    let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
                    assert!(lease.acquire(&mut io).is_err(), "failure {failure}");
                    lease.restore(&mut io).unwrap();
                    assert_eq!(io.prefs, original, "failure {failure}");
                    assert!(!io.menu);
                }
            }
        }
    }

    #[test]
    fn every_restoration_input_failure_stops_immediately() {
        for slot in ["primary", "secondary"] {
            let mut healthy = Model::new();
            healthy.prefs.insert("LastActiveTool".into(), slot.into());
            let mut lease = Lease::prepare(healthy.observe().unwrap()).unwrap().unwrap();
            lease.acquire(&mut healthy).unwrap();
            let first = healthy.count + 1;
            lease.restore(&mut healthy).unwrap();
            let last = healthy.count;
            for before in [false, true] {
                for failure in first..=last {
                    let mut io = Model::new();
                    io.prefs.insert("LastActiveTool".into(), slot.into());
                    let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
                    lease.acquire(&mut io).unwrap();
                    io.fail = Some(failure);
                    io.fail_before = before;
                    assert!(lease.restore(&mut io).is_err());
                    assert_eq!(io.count, failure);
                    assert!(io.checkpoints.last().unwrap().phase.restoring());
                }
            }
        }
    }

    #[test]
    fn unsupported_fine_probe_restores_tool_without_touching_style() {
        let mut io = Model::new();
        io.prefs
            .insert("LastFinelinerv2Color".into(), "ArgbCode".into());
        let original = io.prefs.clone();
        let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
        assert!(lease.acquire(&mut io).is_err());
        lease.restore(&mut io).unwrap();
        assert_eq!(io.prefs, original);
        assert!(io
            .checkpoints
            .iter()
            .all(|r| !r.mutations.color && !r.mutations.width));
    }

    #[test]
    fn journal_failure_prevents_unrecorded_input_and_rollback_guessing() {
        for failure in 1..=6 {
            let mut io = Model::new();
            io.fail_checkpoint = Some(failure);
            let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
            assert!(lease.acquire(&mut io).is_err());
            assert_eq!(io.count, failure - 1);
            let count = io.count;
            let _ = lease.restore(&mut io);
            assert_eq!(io.count, count, "No input after journal failure");
        }
    }

    #[test]
    fn journal_reserves_capacity_and_rejects_incomplete_crash_tail() {
        let path = std::env::temp_dir().join(format!(
            "reader-status-capacity-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut record = Recovery::new(identity(), prefs(), "primary".into()).unwrap();
        let mut journal = Journal::create(&path, &record).unwrap();
        for sequence in 1..RECORD_COUNT - ROLLBACK_RESERVE {
            record.sequence = sequence;
            record.phase = Phase::OpenPrimary;
            record.mutations.menu = true;
            journal.append(&record).unwrap();
        }
        record.sequence += 1;
        assert!(journal.append(&record).is_err());
        for sequence in record.sequence..RECORD_COUNT {
            record.sequence = sequence;
            record.phase = Phase::RestoreClose;
            journal.append(&record).unwrap();
        }
        assert_eq!(Recovery::read(&path).unwrap().sequence, RECORD_COUNT - 1);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"partial\":")
            .unwrap();
        assert!(Recovery::read(&path).is_err());
        assert!(journal.finish().is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn changed_page_refuses_rollback_input() {
        let mut io = Model::new();
        let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
        lease.acquire(&mut io).unwrap();
        io.identity.visit = "1:91".into();
        let count = io.count;
        assert!(lease.restore(&mut io).is_err());
        assert_eq!(io.count, count);
    }

    #[test]
    fn partial_and_unrecognized_popovers_refuse_before_input() {
        let mut io = Model::new();
        let mut state = io.observe().unwrap();
        state.image = fixture(include_bytes!(
            "../../tests/fixtures/status-style/open-fineliner.png"
        ));
        state
            .preferences
            .insert("LastActiveTool".into(), "primary".into());
        // A damaged/partly painted supported border previously looked closed.
        state.image.put_pixel(279, 100, Luma([255]));
        assert!(Lease::prepare(state).unwrap().is_none());
        let mut state = io.observe().unwrap();
        // A docked popover elsewhere on the seam has no pen-menu grid at all.
        for y in 700..850 {
            state.image.put_pixel(61, y, Luma([0]));
        }
        assert!(Lease::prepare(state).unwrap().is_none());
        assert_eq!(io.count, 0);
    }

    #[test]
    fn unchanged_primary_style_restores_without_reopening_menu() {
        let mut io = Model::new();
        for (k, v) in [
            ("LastActiveTool", "primary"),
            ("LastPen", "Finelinerv2"),
            ("LastFinelinerv2Color", "Black"),
            ("LastFinelinerv2Size", "2"),
        ] {
            io.prefs.insert(k.into(), v.into());
        }
        let original = io.prefs.clone();
        let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
        lease.acquire(&mut io).unwrap();
        let count = io.count;
        lease.restore(&mut io).unwrap();
        assert_eq!(count, io.count);
        assert_eq!(io.prefs, original);
        assert!(!io.menu);
    }

    #[test]
    fn recovery_is_exclusive_versioned_and_validated() {
        let path = std::env::temp_dir().join(format!(
            "reader-status-recovery-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let record = Recovery::new(identity(), prefs(), "secondary".into()).unwrap();
        let mut journal = Journal::create(&path, &record).unwrap();
        let initial = fs::read(&path).unwrap();
        assert!(Journal::create(&path, &record).is_err());
        assert_eq!(Recovery::read(&path).unwrap().advisory, prefs());
        fs::write(&path, b"{\"version\":9}").unwrap();
        assert!(Recovery::read(&path).is_err());
        assert!(Journal::create(&path, &record).is_err());
        let mut next = record.clone();
        next.sequence = 1;
        next.phase = Phase::OpenPrimary;
        next.mutations.menu = true;
        assert!(journal.append(&next).is_err());
        fs::write(&path, initial).unwrap();
        assert!(journal.finish().is_err());
        fs::remove_file(path).unwrap();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    SelectPrimary,
    OpenPrimary,
    SelectFine,
    SetColor,
    SetWidth,
    CloseForDrawing,
    RestoreSelectPrimary,
    RestoreOpen,
    RestoreColor,
    RestoreWidth,
    RestorePrimary,
    RestoreClose,
    RestoreSlot,
}
impl Phase {
    fn restoring(self) -> bool {
        matches!(
            self,
            Self::RestoreSelectPrimary
                | Self::RestoreOpen
                | Self::RestoreColor
                | Self::RestoreWidth
                | Self::RestorePrimary
                | Self::RestoreClose
                | Self::RestoreSlot
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutations {
    pub slot: bool,
    pub menu: bool,
    pub primary: bool,
    pub color: bool,
    pub width: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    version: u32,
    sequence: usize,
    pub identity: Identity,
    /// Persisted settings can lag the UI. Never use these as restoration targets.
    pub advisory: Preferences,
    pub original_slot: String,
    pub original_grid: Option<usize>,
    pub original_fine: Option<(usize, usize)>,
    pub phase: Phase,
    pub mutations: Mutations,
}
const RECORD_LIMIT: usize = 32768;
const RECORD_COUNT: usize = 32;
const ROLLBACK_RESERVE: usize = 12;
const JOURNAL_LIMIT: usize = RECORD_LIMIT * RECORD_COUNT;

impl Recovery {
    pub fn new(identity: Identity, advisory: Preferences, original_slot: String) -> Result<Self> {
        let record = Self {
            version: 2,
            sequence: 0,
            identity,
            advisory,
            original_slot,
            original_grid: None,
            original_fine: None,
            phase: Phase::Prepared,
            mutations: Mutations::default(),
        };
        record.validate()?;
        Ok(record)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 2 && self.sequence < RECORD_COUNT,
            "Unsupported status recovery version/sequence"
        );
        for id in [&self.identity.document, &self.identity.page] {
            ensure!(
                id.len() == 36
                    && id
                        .bytes()
                        .enumerate()
                        .all(|(i, b)| if [8, 13, 18, 23].contains(&i) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }),
                "Invalid status recovery identity"
            );
        }
        ensure!(
            !self.identity.visit.is_empty()
                && self.identity.visit.len() <= 80
                && !self.identity.session.is_empty()
                && self.identity.session.len() <= 80,
            "Invalid status recovery revision"
        );
        ensure!(
            self.advisory.len() <= 128
                && self
                    .advisory
                    .iter()
                    .all(|(k, v)| k.len() <= 80 && v.len() <= 80),
            "Status advisory snapshot exceeds bounds"
        );
        ensure!(
            matches!(self.original_slot.as_str(), "primary" | "secondary"),
            "Unsupported actual active slot"
        );
        ensure!(
            self.original_grid.is_none_or(|i| i < 9),
            "Invalid original UI grid"
        );
        ensure!(
            self.original_fine.is_none_or(|(c, w)| c < 9 && w < 3),
            "Invalid original UI Fineliner style"
        );
        ensure!(
            self.original_fine.is_none() || self.original_grid.is_some(),
            "Style snapshot lacks its tool probe"
        );
        ensure!(
            !self.mutations.primary || self.original_grid.is_some(),
            "Missing tool rollback target"
        );
        ensure!(
            !(self.mutations.color || self.mutations.width) || self.original_fine.is_some(),
            "Missing style rollback target"
        );
        let intended = match self.phase {
            Phase::Prepared => self.sequence == 0 && self.mutations == Mutations::default(),
            Phase::SelectPrimary | Phase::RestoreSelectPrimary | Phase::RestoreSlot => {
                self.mutations.slot
            }
            Phase::OpenPrimary
            | Phase::RestoreOpen
            | Phase::CloseForDrawing
            | Phase::RestoreClose => self.mutations.menu,
            Phase::SelectFine | Phase::RestorePrimary => self.mutations.primary,
            Phase::SetColor | Phase::RestoreColor => self.mutations.color,
            Phase::SetWidth | Phase::RestoreWidth => self.mutations.width,
        };
        ensure!(intended, "Recovery phase lacks mutation intent");
        Ok(())
    }
    fn follows(&self, old: &Self) -> Result<()> {
        self.validate()?;
        ensure!(
            self.sequence == old.sequence + 1
                && self.identity == old.identity
                && self.advisory == old.advisory
                && self.original_slot == old.original_slot,
            "Recovery identity/sequence changed"
        );
        ensure!(
            old.original_grid.is_none() || self.original_grid == old.original_grid,
            "Original tool snapshot changed"
        );
        ensure!(
            old.original_fine.is_none() || self.original_fine == old.original_fine,
            "Original style snapshot changed"
        );
        for (before, after) in [
            (old.mutations.slot, self.mutations.slot),
            (old.mutations.menu, self.mutations.menu),
            (old.mutations.primary, self.mutations.primary),
            (old.mutations.color, self.mutations.color),
            (old.mutations.width, self.mutations.width),
        ] {
            ensure!(!before || after, "Recovery mutation flag regressed");
        }
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(JOURNAL_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= JOURNAL_LIMIT && bytes.last() == Some(&b'\n'),
            "Incomplete/oversized status recovery journal"
        );
        let mut last: Option<Self> = None;
        let mut count = 0;
        for line in bytes[..bytes.len() - 1].split(|b| *b == b'\n') {
            count += 1;
            ensure!(
                count <= RECORD_COUNT && line.len() <= RECORD_LIMIT,
                "Status recovery bounds exceeded"
            );
            let record: Self = serde_json::from_slice(line)?;
            record.validate()?;
            if let Some(previous) = &last {
                record.follows(previous)?;
            } else {
                ensure!(
                    record.sequence == 0
                        && record.phase == Phase::Prepared
                        && record.mutations == Mutations::default()
                        && record.original_grid.is_none()
                        && record.original_fine.is_none(),
                    "Invalid initial recovery checkpoint"
                );
            }
            last = Some(record);
        }
        last.context("Empty status recovery journal")
    }
}

pub struct Journal {
    file: fs::File,
    path: std::path::PathBuf,
    last: Recovery,
    bytes: u64,
    failed: bool,
}
impl Journal {
    pub fn create(path: &Path, record: &Recovery) -> Result<Self> {
        record.validate()?;
        ensure!(
            record.sequence == 0
                && record.phase == Phase::Prepared
                && record.original_grid.is_none()
                && record.original_fine.is_none()
                && record.mutations == Mutations::default(),
            "Invalid initial recovery phase"
        );
        let mut options = fs::OpenOptions::new();
        options.read(true).append(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .context("Unresolved status recovery record or unavailable cache")?;
        let mut bytes = serde_json::to_vec(record)?;
        ensure!(
            bytes.len() <= RECORD_LIMIT,
            "Recovery checkpoint exceeds bounds"
        );
        bytes.push(b'\n');
        file.write_all(&bytes)?;
        file.sync_all()?;
        // The directory entry must survive a crash before the first UI input.
        #[cfg(unix)]
        fs::File::open(path.parent().context("Missing recovery parent")?)?.sync_all()?;
        Ok(Self {
            file,
            path: path.to_owned(),
            last: record.clone(),
            bytes: bytes.len() as u64,
            failed: false,
        })
    }
    fn verify_owned(&self) -> Result<()> {
        let current = fs::symlink_metadata(&self.path)?;
        ensure!(
            current.is_file()
                && current.len() == self.bytes
                && self.file.metadata()?.len() == self.bytes,
            "Recovery journal ownership/length changed"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let opened = self.file.metadata()?;
            ensure!(
                current.dev() == opened.dev()
                    && current.ino() == opened.ino()
                    && current.mode() & 0o077 == 0,
                "Recovery journal ownership/permissions changed"
            );
        }
        ensure!(
            Recovery::read(&self.path)? == self.last,
            "Recovery journal changed outside this lease"
        );
        Ok(())
    }
    pub fn append(&mut self, record: &Recovery) -> Result<()> {
        ensure!(!self.failed, "Recovery journal I/O already failed");
        record.follows(&self.last)?;
        let mut bytes = serde_json::to_vec(record)?;
        ensure!(
            bytes.len() <= RECORD_LIMIT,
            "Recovery checkpoint exceeds bounds"
        );
        bytes.push(b'\n');
        let count = record.sequence + 1;
        let limit = if record.phase.restoring() {
            RECORD_COUNT
        } else {
            RECORD_COUNT - ROLLBACK_RESERVE
        };
        let byte_limit = if record.phase.restoring() {
            JOURNAL_LIMIT
        } else {
            JOURNAL_LIMIT - ROLLBACK_RESERVE * (RECORD_LIMIT + 1)
        };
        ensure!(
            count <= limit && self.bytes + bytes.len() as u64 <= byte_limit as u64,
            "Status journal capacity reserved for rollback"
        );
        let written = (|| -> Result<()> {
            self.verify_owned()?;
            self.file.write_all(&bytes)?;
            self.file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = written {
            self.failed = true;
            return Err(error);
        }
        self.bytes += bytes.len() as u64;
        self.last = record.clone();
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        ensure!(
            !self.failed,
            "Recovery journal I/O failed; retain evidence for deliberate recovery"
        );
        self.verify_owned()?;
        let parent = self
            .path
            .parent()
            .context("Missing recovery parent")?
            .to_owned();
        drop(self.file);
        fs::remove_file(&self.path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        #[cfg(not(unix))]
        let _ = parent;
        Ok(())
    }
}

#[cfg(test)]
fn fine_style(prefs: &Preferences) -> Result<(usize, usize)> {
    let color = prefs
        .get("LastFinelinerv2Color")
        .context("Missing saved Fineliner color")?;
    let color = COLORS
        .iter()
        .position(|c| *c == color)
        .context("Unsupported saved Fineliner color")?;
    let size = prefs
        .get("LastFinelinerv2Size")
        .context("Missing saved Fineliner size")?
        .parse::<usize>()?;
    ensure!((1..=3).contains(&size), "Unsupported saved Fineliner size");
    Ok((color, size - 1))
}

fn dark(image: &GrayImage, x: u32, y: u32) -> bool {
    image.get_pixel(x, y).0[0] < 32
}

fn selected(image: &GrayImage, x: u32, y: u32) -> bool {
    [
        (x - 23, y - 23),
        (x + 23, y - 23),
        (x - 23, y + 23),
        (x + 23, y + 23),
    ]
    .into_iter()
    .all(|(x, y)| dark(image, x, y))
}

fn unique_selection(image: &GrayImage, points: &[(u32, u32)]) -> Option<usize> {
    let found: Vec<_> = points
        .iter()
        .enumerate()
        .filter(|(_, (x, y))| selected(image, *x, *y))
        .map(|(i, _)| i)
        .collect();
    (found.len() == 1).then(|| found[0])
}

pub const GRID: [(u32, u32); 9] = [
    (110, 135),
    (171, 135),
    (232, 135),
    (110, 197),
    (171, 197),
    (232, 197),
    (110, 260),
    (171, 260),
    (232, 260),
];
const WIDTHS: [(u32, u32); 3] = [(110, 382), (171, 382), (232, 382)];
const PALETTE: [(u32, u32); 9] = [
    (110, 479),
    (171, 479),
    (232, 479),
    (110, 540),
    (171, 540),
    (232, 540),
    (110, 601),
    (171, 601),
    (232, 601),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controls {
    pub slot: &'static str,
    pub menu: bool,
    pub grid: Option<usize>,
    pub fine: Option<(usize, usize)>,
}

pub fn controls(image: &GrayImage) -> Option<Controls> {
    if image.dimensions() != (768, 1024) {
        return None;
    }
    // Avoid the upper-right color dot: Yellow/White remain light inside a
    // selected black tile and do not mean the slot is unselected.
    let selected_slot = |y: u32| {
        [(5, y - 10), (55, y + 10), (5, y + 20), (55, y + 23)]
            .into_iter()
            .all(|(x, y)| dark(image, x, y))
    };
    let slot = match (selected_slot(91), selected_slot(153)) {
        (true, false) => "primary",
        (false, true) => "secondary",
        _ => return None,
    };
    // The supported menu has a continuous top/right border and a shared tool
    // grid. Unknown popovers fail closed instead of being clicked away.
    let border = (70..270).all(|x| dark(image, x, 61)) && (70..300).all(|y| dark(image, 279, y));
    let grid = border.then(|| unique_selection(image, &GRID)).flatten();
    if border && grid.is_none() {
        return None;
    }
    if !border {
        // Require the exposed page-side toolbar seam to be clear. Other or
        // partially rendered docked popovers may retain the selected pen tile
        // while lacking this menu's complete top/right border. Nearby page ink
        // can conservatively suppress status; it is never removed for this probe.
        if !(62..1000).all(|y| (61..65).all(|x| image.get_pixel(x, y).0[0] >= 248))
            || GRID.iter().any(|&(x, y)| selected(image, x, y))
        {
            return None;
        }
    }
    let fine = if grid == Some(1) && (70..270).all(|x| dark(image, x, 430)) {
        unique_selection(image, &PALETTE).zip(unique_selection(image, &WIDTHS))
    } else {
        None
    };
    Some(Controls {
        slot,
        menu: border,
        grid,
        fine,
    })
}

#[derive(Clone)]
pub struct Observation {
    pub identity: Identity,
    pub preferences: Preferences,
    pub image: GrayImage,
}

pub trait StyleIo {
    fn observe(&mut self) -> Result<Observation>;
    fn checkpoint(&mut self, record: &Recovery) -> Result<()>;
    fn press(&mut self, point: (u32, u32)) -> Result<()>;
}

pub struct Lease {
    pub recovery: Recovery,
    baseline: GrayImage,
}
impl Lease {
    pub fn prepare(observed: Observation) -> Result<Option<Self>> {
        let Some(ui) = controls(&observed.image) else {
            return Ok(None);
        };
        if ui.menu {
            return Ok(None);
        }
        let recovery = Recovery::new(observed.identity, observed.preferences, ui.slot.to_owned())?;
        Ok(Some(Self {
            recovery,
            baseline: observed.image,
        }))
    }
    fn observe(&self, io: &mut impl StyleIo) -> Result<(Observation, Controls)> {
        let state = io.observe()?;
        ensure!(
            state.identity == self.recovery.identity,
            "Status page/session changed; restoration stopped"
        );
        ensure!(
            state.image.dimensions() == self.baseline.dimensions(),
            "Status image dimensions changed"
        );
        ensure!(
            (0..1024).all(|y| (0..768).all(|x| {
                x < 61
                    || (x < 280 && (61..651).contains(&y))
                    || (x >= 686 && y >= 922)
                    || state.image.get_pixel(x, y).0[0].abs_diff(self.baseline.get_pixel(x, y).0[0])
                        <= 8
            })),
            "Status page image changed; restoration stopped"
        );
        let ui = controls(&state.image).context("Status toolbar layout changed")?;
        Ok((state, ui))
    }
    fn transition(
        &mut self,
        io: &mut impl StyleIo,
        phase: Phase,
        point: (u32, u32),
        expected: impl Fn(&Controls) -> bool,
    ) -> Result<()> {
        self.observe(io)?;
        let m = &mut self.recovery.mutations;
        match phase {
            Phase::SelectPrimary | Phase::RestoreSelectPrimary | Phase::RestoreSlot => {
                m.slot = true
            }
            Phase::OpenPrimary
            | Phase::RestoreOpen
            | Phase::CloseForDrawing
            | Phase::RestoreClose => m.menu = true,
            Phase::SelectFine | Phase::RestorePrimary => m.primary = true,
            Phase::SetColor | Phase::RestoreColor => m.color = true,
            Phase::SetWidth | Phase::RestoreWidth => m.width = true,
            Phase::Prepared => anyhow::bail!("Prepared phase cannot send input"),
        }
        self.recovery.phase = phase;
        self.recovery.sequence += 1;
        // Record intent durably BEFORE input: a failed press may have acted.
        io.checkpoint(&self.recovery)?;
        io.press(point)?;
        for _ in 0..5 {
            let (_, ui) = self.observe(io)?;
            if expected(&ui) {
                return Ok(());
            }
        }
        anyhow::bail!("Status controls did not converge after input")
    }
    fn primary_menu(&mut self, io: &mut impl StyleIo, restoring: bool) -> Result<()> {
        let (_, ui) = self.observe(io)?;
        if ui.slot != "primary" {
            ensure!(!ui.menu, "Unexpected secondary menu");
            self.transition(
                io,
                if restoring {
                    Phase::RestoreSelectPrimary
                } else {
                    Phase::SelectPrimary
                },
                (30, 90),
                |u| u.slot == "primary" && !u.menu,
            )?;
        }
        if !self.observe(io)?.1.menu {
            self.transition(
                io,
                if restoring {
                    Phase::RestoreOpen
                } else {
                    Phase::OpenPrimary
                },
                (30, 90),
                |u| u.slot == "primary" && u.menu,
            )?;
        }
        Ok(())
    }
    fn fine(&self, io: &mut impl StyleIo) -> Result<(usize, usize)> {
        let (_, ui) = self.observe(io)?;
        ensure!(
            ui.slot == "primary" && ui.grid == Some(1),
            "Fineliner menu not selected"
        );
        ui.fine.context("Unsupported actual Fineliner color/width")
    }
    fn known_fine(&self, io: &mut impl StyleIo) -> Result<(usize, usize)> {
        let current = self.fine(io)?;
        let original = self
            .recovery
            .original_fine
            .context("Missing actual Fineliner snapshot")?;
        ensure!(
            (current.0 == original.0 || current.0 == 0)
                && (current.1 == original.1 || current.1 == 1),
            "Unexpected style change during lease"
        );
        Ok(current)
    }
    pub fn acquire(&mut self, io: &mut impl StyleIo) -> Result<()> {
        self.primary_menu(io, false)?;
        let (_, ui) = self.observe(io)?;
        self.recovery.original_grid = Some(ui.grid.context("Missing actual primary tool grid")?);
        if ui.grid != Some(1) {
            self.transition(io, Phase::SelectFine, GRID[1], |u| u.grid == Some(1))?;
        }
        self.recovery.original_fine = Some(self.fine(io)?);
        if self.known_fine(io)?.0 != 0 {
            self.transition(io, Phase::SetColor, PALETTE[0], |u| {
                u.fine.is_some_and(|f| f.0 == 0)
            })?;
        }
        if self.known_fine(io)?.1 != 1 {
            self.transition(io, Phase::SetWidth, WIDTHS[1], |u| {
                u.fine.is_some_and(|f| f.1 == 1)
            })?;
        }
        ensure!(self.fine(io)? == (0, 1), "Temporary style not verified");
        self.transition(io, Phase::CloseForDrawing, (30, 90), |u| {
            !u.menu && u.slot == "primary"
        })
    }
    pub fn restore(&mut self, io: &mut impl StyleIo) -> Result<()> {
        let (state, ui) = self.observe(io)?;
        let m = self.recovery.mutations.clone();
        if !m.slot && !m.menu && !m.primary && !m.color && !m.width {
            ensure!(
                !ui.menu
                    && ui.slot == self.recovery.original_slot
                    && self.original_controls(&state.image),
                "Unmodified original slot not verified"
            );
            return Ok(());
        }
        // This fast path uses captured actual UI values and a fresh original
        // closed slot, never equality with a potentially stale document file.
        if self.recovery.original_grid == Some(1)
            && self.recovery.original_fine == Some((0, 1))
            && self.recovery.original_slot == "primary"
            && !m.color
            && !m.width
            && !m.primary
            && !ui.menu
            && ui.slot == "primary"
            && self.original_controls(&state.image)
        {
            return Ok(());
        }
        if let Some(original) = self.recovery.original_grid {
            self.primary_menu(io, true)?;
            let (_, ui) = self.observe(io)?;
            ensure!(
                ui.grid == Some(1) || ui.grid == Some(original),
                "Unexpected primary tool during rollback"
            );
            if m.color || m.width {
                ensure!(ui.grid == Some(1), "Modified Fineliner no longer selected");
                let desired = self
                    .recovery
                    .original_fine
                    .context("Missing captured style for rollback")?;
                if m.color && self.known_fine(io)?.0 != desired.0 {
                    self.transition(io, Phase::RestoreColor, PALETTE[desired.0], |u| {
                        u.fine.is_some_and(|f| f.0 == desired.0)
                    })?;
                }
                if m.width && self.known_fine(io)?.1 != desired.1 {
                    self.transition(io, Phase::RestoreWidth, WIDTHS[desired.1], |u| {
                        u.fine.is_some_and(|f| f.1 == desired.1)
                    })?;
                }
                ensure!(
                    self.fine(io)? == desired,
                    "Actual Fineliner preferences not restored"
                );
            } else if ui.grid == Some(1) {
                if let Some(desired) = self.recovery.original_fine {
                    ensure!(
                        self.fine(io)? == desired,
                        "Unmodified Fineliner preferences changed"
                    );
                }
                // No color/width input was attempted. An unsupported style or
                // failed probe must not write any dimension from advisory data.
            }
            if self.observe(io)?.1.grid != Some(original) {
                self.transition(io, Phase::RestorePrimary, GRID[original], |u| {
                    u.grid == Some(original)
                })?;
            }
            ensure!(
                self.observe(io)?.1.grid == Some(original),
                "Original primary grid not restored"
            );
        }
        // Before a grid was captured, only our positively identified primary
        // menu and the original active slot can be restored; never guess a tool.
        let (_, ui) = self.observe(io)?;
        if ui.menu {
            ensure!(
                m.menu && ui.slot == "primary",
                "Refuse to close unowned menu"
            );
            self.transition(io, Phase::RestoreClose, (30, 90), |u| {
                !u.menu && u.slot == "primary"
            })?;
        }
        if self.recovery.original_slot == "secondary" && self.observe(io)?.1.slot != "secondary" {
            self.transition(io, Phase::RestoreSlot, (30, 150), |u| {
                u.slot == "secondary" && !u.menu
            })?;
        }
        let (state, ui) = self.observe(io)?;
        ensure!(
            !ui.menu
                && ui.slot == self.recovery.original_slot
                && self.original_controls(&state.image),
            "Original active slot not restored"
        );
        Ok(())
    }
    fn original_controls(&self, image: &GrayImage) -> bool {
        (61..184).all(|y| {
            (0..61).all(|x| {
                image.get_pixel(x, y).0[0].abs_diff(self.baseline.get_pixel(x, y).0[0]) <= 8
            })
        })
    }
}
