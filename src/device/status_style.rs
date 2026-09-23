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
    time::Duration,
};

pub type Preferences = BTreeMap<String, String>;

/// Shared native/simulator latch: losing wait input ownership cannot be undone
/// by a later successful poll or an attempted style rollback.
#[derive(Default)]
pub(super) struct WaitCancellation {
    cancelled: bool,
}
impl WaitCancellation {
    pub(super) fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled,
            "Status input ownership was cancelled; further input stopped"
        );
        Ok(())
    }
    pub(super) fn record(&mut self, result: Result<()>) -> Result<()> {
        self.check()?;
        if result.is_err() {
            self.cancelled = true;
        }
        result
    }
}
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

    struct CurrentIo {
        state: Observation,
        records: Vec<Recovery>,
        journal: Option<Journal>,
        fail_checkpoint: bool,
        change_after_checkpoint: bool,
        cancelled: bool,
    }
    impl StyleIo for CurrentIo {
        fn observe(&mut self) -> Result<Observation> {
            Ok(self.state.clone())
        }
        fn checkpoint(&mut self, record: &Recovery) -> Result<()> {
            ensure!(!self.fail_checkpoint, "injected journal failure");
            record.validate()?;
            if let Some(journal) = &mut self.journal {
                journal.append(record)?;
            }
            self.records.push(record.clone());
            if self.change_after_checkpoint {
                self.state.identity.visit = "changed".into();
            }
            Ok(())
        }
        fn press(&mut self, _: (u32, u32)) -> Result<()> {
            panic!("current-tool path sent menu input")
        }
        fn monotonic(&self) -> Duration {
            Duration::ZERO
        }
        fn pace(&mut self, _: Duration) {
            panic!("current-tool path waited for a menu")
        }
        fn begin_wait(&mut self) -> Result<()> {
            self.check_wait()
        }
        fn check_wait(&mut self) -> Result<()> {
            ensure!(!self.cancelled, "cancelled");
            Ok(())
        }
        fn end_wait(&mut self) {}
    }
    fn current_io() -> CurrentIo {
        CurrentIo {
            state: Observation {
                identity: identity(),
                preferences: prefs(),
                image: closed(),
            },
            records: vec![],
            journal: None,
            fail_checkpoint: false,
            change_after_checkpoint: false,
            cancelled: false,
        }
    }
    #[test]
    fn current_tool_candidate_uses_closed_pixels_not_saved_preferences() {
        let io = current_io(); // Deliberately stale Highlighter/Red saved settings.
        assert_eq!(closed_black_fineliner(&io.state.image), Some("primary"));
        for bytes in [
            include_bytes!("../../tests/fixtures/status-style/closed-highlighter.png").as_slice(),
            include_bytes!("../../tests/fixtures/status-style/closed-secondary.png").as_slice(),
            include_bytes!("../../tests/fixtures/status-style/closed-secondary-highlighter.png")
                .as_slice(),
            include_bytes!("../../tests/fixtures/status-style/open-fineliner.png").as_slice(),
        ] {
            assert_eq!(
                closed_black_fineliner(&image::load_from_memory(bytes).unwrap().to_luma8()),
                None
            );
        }
        let mut changed = io.state.image.clone();
        for y in 70..77 {
            for x in 46..53 {
                changed.put_pixel(x, y, Luma([255]));
            }
        }
        assert_eq!(closed_black_fineliner(&changed), None);
        // Model only: a moved active tile is not native secondary-slot proof.
        let mut secondary = io.state.image.clone();
        for y in 61..123 {
            for x in 0..61 {
                secondary.put_pixel(x, y + 62, *io.state.image.get_pixel(x, y));
                secondary.put_pixel(x, y, Luma([255]));
            }
        }
        assert_eq!(closed_black_fineliner(&secondary), None);
        for (bytes, expected) in [
            (
                include_bytes!("../../tests/fixtures/status-style/current-secondary-fine.png")
                    .as_slice(),
                Some("secondary"),
            ),
            (
                include_bytes!("../../tests/fixtures/status-style/current-thin-fine.png")
                    .as_slice(),
                Some("primary"),
            ),
            (
                include_bytes!("../../tests/fixtures/status-style/current-white-fine.png")
                    .as_slice(),
                None,
            ),
        ] {
            let mut state = io.state.image.clone();
            replace(
                &mut state,
                &image::load_from_memory(bytes).unwrap().to_luma8(),
                0,
                0,
            );
            assert_eq!(closed_black_fineliner(&state), expected);
        }
    }
    #[test]
    fn current_tool_journal_and_cleanup_issue_no_menu_input() {
        let mut io = current_io();
        let mut lease = Lease::prepare_current(io.state.clone(), false)
            .unwrap()
            .unwrap();
        assert!(lease.verify_current(&mut io).is_err());
        let path =
            std::env::temp_dir().join(format!("rb-current-tool-{}.jsonl", std::process::id()));
        io.journal = Some(Journal::create(&path, &lease.recovery).unwrap());
        lease.acquire(&mut io).unwrap();
        assert_eq!(Recovery::read(&path).unwrap().phase, Phase::CurrentInk);
        assert!(lease
            .transition(&mut io, Phase::OpenPrimary, (30, 90), |_| true)
            .is_err());
        lease.verify_current(&mut io).unwrap();
        lease.prepare_cleanup(&mut io).unwrap();
        assert!(lease.verify_current(&mut io).is_err());
        assert_eq!(Recovery::read(&path).unwrap().phase, Phase::PendingCleanup);
        lease.finish_cleanup(&mut io).unwrap();
        io.journal.take().unwrap().finish().unwrap();
        assert!(!path.exists());
        assert!(io
            .records
            .iter()
            .all(|r| r.mutations == Mutations::default() && r.original_fine.is_none()));
    }
    #[test]
    fn current_tool_stops_on_checkpoint_owner_content_and_input_faults() {
        for fault in 0..4 {
            let mut io = current_io();
            let mut lease = Lease::prepare_current(io.state.clone(), false)
                .unwrap()
                .unwrap();
            match fault {
                0 => io.fail_checkpoint = true,
                1 => io.change_after_checkpoint = true,
                2 => io.state.image.put_pixel(200, 300, Luma([0])),
                _ => io.cancelled = true,
            }
            assert!(lease.acquire(&mut io).is_err());
        }
        let mut io = current_io();
        let mut lease = Lease::prepare_current(io.state.clone(), false)
            .unwrap()
            .unwrap();
        lease.acquire(&mut io).unwrap();
        io.state.image.put_pixel(200, 100, Luma([0])); // Old menu exclusion must not apply.
        assert!(lease.verify_current(&mut io).is_err());
    }
    #[test]
    fn current_tool_cleanup_requires_durable_intent_and_unchanged_input() {
        for fault in 0..3 {
            let mut io = current_io();
            let mut lease = Lease::prepare_current(io.state.clone(), false)
                .unwrap()
                .unwrap();
            lease.acquire(&mut io).unwrap();
            match fault {
                0 => io.fail_checkpoint = true,
                1 => io.change_after_checkpoint = true,
                _ => io.cancelled = true,
            }
            assert!(lease.prepare_cleanup(&mut io).is_err());
            assert!(lease.finish_cleanup(&mut io).is_err());
        }
        let mut io = current_io();
        let mut lease = Lease::prepare_current(io.state.clone(), false)
            .unwrap()
            .unwrap();
        lease.acquire(&mut io).unwrap();
        lease.prepare_cleanup(&mut io).unwrap();
        io.state.image.put_pixel(720, 960, Luma([0]));
        assert!(lease.finish_cleanup(&mut io).is_err());
        let mut changed_version = lease.recovery.clone();
        changed_version.version = 2;
        assert!(changed_version.validate().is_err());
        // Exercise the landmark redraw allowance without allowing it to hide
        // changed neighbors immediately outside the blank erasure footprint.
        io.state.image = closed();
        lease.viewport = CleanupViewport::Landmarks;
        io.state.image.put_pixel(685, 960, Luma([0]));
        assert!(lease.finish_cleanup(&mut io).is_err());
        io.state.image = closed();
        io.state.image.put_pixel(30, 350, Luma([0]));
        assert!(lease.finish_cleanup(&mut io).is_err());
    }
    #[test]
    fn current_tool_only_allows_observed_undo_chrome() {
        let before = image::load_from_memory(include_bytes!(
            "../../tests/fixtures/status-style/current-toolbar-before.png"
        ))
        .unwrap()
        .to_luma8();
        let active = image::load_from_memory(include_bytes!(
            "../../tests/fixtures/status-style/current-toolbar-active.png"
        ))
        .unwrap()
        .to_luma8();
        let mut io = current_io();
        replace(&mut io.state.image, &before, 0, 0);
        let mut lease = Lease::prepare_current(io.state.clone(), false)
            .unwrap()
            .unwrap();
        lease.acquire(&mut io).unwrap();
        replace(&mut io.state.image, &active, 0, 0);
        lease.verify_current(&mut io).unwrap();
        for (x, y) in [(30, 210), (30, 350), (49, 73), (760, 960), (720, 1000)] {
            let old = *io.state.image.get_pixel(x, y);
            io.state.image.put_pixel(x, y, Luma([255 - old.0[0]]));
            assert!(lease.verify_current(&mut io).is_err(), "changed {x},{y}");
            io.state.image.put_pixel(x, y, old);
        }
    }

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
        journal: Option<Journal>,
        fail_checkpoint: Option<usize>,
        fail_before: bool,
        menu: bool,
        count: usize,
        fail: Option<usize>,
        identity: Identity,
        canvas: Option<GrayImage>,
        fail_cleanup_checkpoint: bool,
        clock: Duration,
        observations: usize,
        change_style_at_observation: Option<usize>,
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
                journal: None,
                fail_checkpoint: None,
                fail_before: false,
                menu: false,
                count: 0,
                fail: None,
                identity: identity(),
                canvas: None,
                fail_cleanup_checkpoint: false,
                clock: Duration::ZERO,
                observations: 0,
                change_style_at_observation: None,
            }
        }
    }
    impl StyleIo for Model {
        fn monotonic(&self) -> Duration {
            self.clock
        }
        fn pace(&mut self, duration: Duration) {
            self.clock += duration;
        }
        fn begin_wait(&mut self) -> Result<()> {
            Ok(())
        }
        fn check_wait(&mut self) -> Result<()> {
            Ok(())
        }
        fn end_wait(&mut self) {}
        fn checkpoint(&mut self, record: &Recovery) -> Result<()> {
            ensure!(
                !(self.fail_cleanup_checkpoint && record.phase == Phase::PendingCleanup),
                "Injected cleanup checkpoint failure"
            );
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
            if let Some(journal) = self.journal.as_mut() {
                journal.append(record)?;
            }
            self.checkpoints.push(record.clone());
            Ok(())
        }
        fn observe(&mut self) -> Result<Observation> {
            self.observations += 1;
            if self.change_style_at_observation == Some(self.observations) {
                self.prefs
                    .insert("LastFinelinerv2Color".into(), "Blue".into());
            }
            let mut image = self.canvas.clone().unwrap_or_else(closed);
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
        assert!(Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .is_none());
        let mut state = io.observe().unwrap();
        state.image = GrayImage::from_pixel(768, 1024, Luma([255]));
        assert!(Lease::prepare(state, false).unwrap().is_none());
        assert_eq!(io.count, 0);
    }

    struct DelayedTransition {
        model: Model,
        before: Observation,
        pending_reads: usize,
        reads: usize,
        observation_cost: Duration,
        waiting: bool,
        guard_calls: usize,
        cancel_at: Option<usize>,
        wrong_owner: bool,
        cancellation: WaitCancellation,
        fail_open: bool,
    }
    impl DelayedTransition {
        fn new(pending_reads: usize, observation_cost: Duration) -> Self {
            let mut model = Model::new();
            let before = model.observe().unwrap();
            Self {
                model,
                before,
                pending_reads,
                reads: 0,
                observation_cost,
                waiting: false,
                guard_calls: 0,
                cancel_at: None,
                wrong_owner: false,
                cancellation: WaitCancellation::default(),
                fail_open: false,
            }
        }
        fn run(&mut self) -> Result<()> {
            let mut lease = Lease::prepare(self.before.clone(), false)?.unwrap();
            lease.transition(self, Phase::SelectPrimary, (30, 90), |ui| {
                ui.slot == "primary" && !ui.menu
            })
        }
    }
    impl StyleIo for DelayedTransition {
        fn monotonic(&self) -> Duration {
            self.model.clock
        }
        fn pace(&mut self, duration: Duration) {
            self.model.clock += duration;
        }
        fn begin_wait(&mut self) -> Result<()> {
            self.cancellation.record(if self.fail_open {
                Err(anyhow::anyhow!("Observer unavailable"))
            } else {
                Ok(())
            })?;
            self.waiting = true;
            Ok(())
        }
        fn check_wait(&mut self) -> Result<()> {
            self.guard_calls += 1;
            self.cancellation
                .record(if self.cancel_at == Some(self.guard_calls) {
                    Err(anyhow::anyhow!("Cancelled input"))
                } else {
                    Ok(())
                })
        }
        fn end_wait(&mut self) {
            self.waiting = false;
        }
        fn checkpoint(&mut self, record: &Recovery) -> Result<()> {
            self.model.checkpoint(record)
        }
        fn press(&mut self, point: (u32, u32)) -> Result<()> {
            self.model.press(point)
        }
        fn observe(&mut self) -> Result<Observation> {
            self.cancellation.check()?;
            if !self.waiting {
                return self.model.observe();
            }
            self.reads += 1;
            self.model.clock += self.observation_cost;
            let mut observed = if self.reads <= self.pending_reads {
                self.before.clone()
            } else {
                self.model.observe()?
            };
            if self.wrong_owner {
                observed.identity.visit = "changed".into();
            }
            Ok(observed)
        }
    }

    #[test]
    fn transition_waits_for_fresh_predicate_without_repeating_input() {
        for delayed in [0, 1, 7] {
            let mut io = DelayedTransition::new(delayed, Duration::from_millis(1));
            io.run().unwrap();
            assert_eq!(io.reads, delayed + 1);
            assert_eq!(io.model.count, 1);
            assert_eq!(io.model.checkpoints.len(), 1);
            assert_eq!(
                io.model.clock,
                Duration::from_millis((delayed * 51 + 1) as u64)
            );
            assert!(!io.waiting);
        }
    }

    #[test]
    fn transition_timeout_and_late_success_never_retry_input() {
        for (pending, cost) in [
            (usize::MAX, Duration::from_millis(500)),
            (0, Duration::from_secs(5)),
        ] {
            let mut io = DelayedTransition::new(pending, cost);
            assert!(io.run().is_err());
            assert_eq!(io.model.count, 1);
            assert_eq!(io.model.checkpoints.len(), 1);
            assert!(!io.waiting);
            assert!(io.model.clock >= Duration::from_secs(5));
            assert!(io.model.clock < Duration::from_millis(5500));
        }
    }

    #[test]
    fn transition_cancellation_and_changed_owner_stop_on_both_sides_of_capture() {
        for cancel_at in [Some(1), Some(2), None] {
            let mut io = DelayedTransition::new(0, Duration::from_millis(1));
            io.cancel_at = cancel_at;
            io.wrong_owner = cancel_at.is_none();
            assert!(io.run().is_err());
            assert_eq!(io.model.count, 1);
            assert_eq!(io.reads, usize::from(cancel_at != Some(1)));
            assert!(!io.waiting);
        }
    }

    #[test]
    fn wait_cancellation_blocks_acquisition_rollback_and_retains_real_journal() {
        for fail_open in [false, true] {
            let mut io = DelayedTransition::new(0, Duration::from_millis(1));
            io.fail_open = fail_open;
            io.cancel_at = Some(2); // cancellation after the first fresh capture
            let mut lease = Lease::prepare(io.before.clone(), false).unwrap().unwrap();
            let path = std::env::temp_dir().join(format!(
                "reader-wait-cancel-{}-{}-{}.json",
                std::process::id(),
                fail_open,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            io.model.journal = Some(Journal::create(&path, &lease.recovery).unwrap());
            assert!(lease.acquire(&mut io).is_err());
            let after_failure = fs::read(&path).unwrap();
            assert_eq!(io.model.count, 1);
            // Even a later good observer result cannot restore ownership.
            io.fail_open = false;
            io.cancel_at = None;
            assert!(io.begin_wait().is_err());
            assert!(lease.restore(&mut io).is_err());
            assert!(lease.prepare_cleanup(&mut io).is_err());
            assert!(lease.finish_cleanup(&mut io).is_err());
            assert_eq!(io.model.count, 1);
            assert_eq!(fs::read(&path).unwrap(), after_failure);
            assert_eq!(Recovery::read(&path).unwrap().sequence, 1);
            assert!(Journal::create(&path, &lease.recovery).is_err());
            drop(io.model.journal.take());
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn native_redraw_requires_landmarks_and_preserves_strict_viewport_checks() {
        let before = fixture(include_bytes!(
            "../../tests/fixtures/status-style/paper-before.png"
        ));
        let after = fixture(include_bytes!(
            "../../tests/fixtures/status-style/paper-after-erase.png"
        ));
        assert_eq!(
            CleanupViewport::classify(&before),
            Some(CleanupViewport::Landmarks)
        );
        assert!(CleanupViewport::Landmarks.unchanged(&before, &after));
        assert!(!CleanupViewport::Blank.unchanged(&before, &after));
        for (dx, dy) in [(1, 0), (0, 1)] {
            let mut shifted = GrayImage::from_pixel(768, 1024, Luma([255]));
            replace(&mut shifted, &before, dx, dy);
            assert!(!CleanupViewport::Landmarks.unchanged(&before, &shifted));
        }
        let zoom =
            image::imageops::resize(&before, 776, 1034, image::imageops::FilterType::Nearest);
        let zoom = image::imageops::crop_imm(&zoom, 4, 5, 768, 1024).to_image();
        assert!(!CleanupViewport::Landmarks.unchanged(&before, &zoom));
        let mut changed = after.clone();
        changed.put_pixel(500, 900, Luma([0]));
        assert!(!CleanupViewport::Landmarks.unchanged(&before, &changed));
        for (x, y) in [(96, 128), (416, 128), (96, 576)] {
            let mut missing = before.clone();
            replace(
                &mut missing,
                &GrayImage::from_pixel(256, 256, Luma([255])),
                x,
                y,
            );
            assert_eq!(CleanupViewport::classify(&missing), None);
        }
    }

    #[test]
    fn sparse_or_confined_content_declines_before_tool_input() {
        for (x, y, w, h) in [
            (600, 840, 20, 20),
            (200, 200, 1, 1),
            (100, 100, 1, 700),
            (100, 100, 500, 1),
            (720, 950, 10, 10),
        ] {
            let mut io = Model::new();
            let mut state = io.observe().unwrap();
            replace(
                &mut state.image,
                &GrayImage::from_pixel(w, h, Luma([0])),
                x,
                y,
            );
            assert!(Lease::prepare(state, false).unwrap().is_none());
            assert_eq!(io.count, 0);
        }
        let blank = closed();
        assert_eq!(
            CleanupViewport::classify(&blank),
            Some(CleanupViewport::Blank)
        );
        let mut changed = blank.clone();
        changed.put_pixel(600, 900, Luma([0]));
        assert!(!CleanupViewport::Blank.unchanged(&blank, &changed));
    }

    #[test]
    fn paper_changes_still_refuse_before_restoration_input() {
        let mut io = Model::new();
        io.canvas = Some(fixture(include_bytes!(
            "../../tests/fixtures/status-style/paper-before.png"
        )));
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        lease.acquire(&mut io).unwrap();
        io.canvas = Some(fixture(include_bytes!(
            "../../tests/fixtures/status-style/paper-after-erase.png"
        )));
        let presses = io.count;
        assert!(lease.prepare_cleanup(&mut io).is_err());
        assert_eq!(presses, io.count);
        assert!(!lease.cleanup_pending());
    }

    #[test]
    fn pending_cleanup_verifies_original_tools_without_later_input() {
        let mut io = Model::new();
        let original = io.prefs.clone();
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "reader-pending-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        io.journal = Some(Journal::create(&path, &lease.recovery).unwrap());
        lease.acquire(&mut io).unwrap();
        lease.prepare_cleanup(&mut io).unwrap();
        assert_eq!(io.prefs, original);
        assert_eq!(io.checkpoints.last().unwrap().phase, Phase::PendingCleanup);
        assert_eq!(Recovery::read(&path).unwrap().phase, Phase::PendingCleanup);
        assert!(Journal::create(&path, &lease.recovery).is_err());
        let presses = io.count;
        lease.finish_cleanup(&mut io).unwrap();
        assert!(lease.restore(&mut io).is_err());
        assert_eq!(presses, io.count);
        io.identity.session = "changed".into();
        assert!(lease.finish_cleanup(&mut io).is_err());
        io.identity = identity();
        io.menu = true;
        assert!(lease.finish_cleanup(&mut io).is_err());
        io.menu = false;
        io.prefs.insert("LastActiveTool".into(), "primary".into());
        assert!(lease.finish_cleanup(&mut io).is_err());
        assert_eq!(presses, io.count);
        assert!(path.exists());
        drop(io.journal.take());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_pending_cleanup_checkpoint_retains_journal_and_forbids_finish() {
        let mut io = Model::new();
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "reader-cleanup-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        io.journal = Some(Journal::create(&path, &lease.recovery).unwrap());
        lease.acquire(&mut io).unwrap();
        io.fail_cleanup_checkpoint = true;
        assert!(lease.prepare_cleanup(&mut io).is_err());
        let presses = io.count;
        assert!(lease.finish_cleanup(&mut io).is_err());
        assert!(lease.restore(&mut io).is_err());
        assert_eq!(presses, io.count);
        assert!(path.exists());
        assert_ne!(Recovery::read(&path).unwrap().phase, Phase::PendingCleanup);
        drop(io.journal.take());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn highlighter_secondary_and_nondefault_fine_restore_exactly() {
        let mut io = Model::new();
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
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
                    let mut lease = Lease::prepare(io.observe().unwrap(), false)
                        .unwrap()
                        .unwrap();
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
            let mut lease = Lease::prepare(healthy.observe().unwrap(), false)
                .unwrap()
                .unwrap();
            lease.acquire(&mut healthy).unwrap();
            let first = healthy.count + 1;
            lease.restore(&mut healthy).unwrap();
            let last = healthy.count;
            for before in [false, true] {
                for failure in first..=last {
                    let mut io = Model::new();
                    io.prefs.insert("LastActiveTool".into(), slot.into());
                    let mut lease = Lease::prepare(io.observe().unwrap(), false)
                        .unwrap()
                        .unwrap();
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
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
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
            let mut lease = Lease::prepare(io.observe().unwrap(), false)
                .unwrap()
                .unwrap();
            assert!(lease.acquire(&mut io).is_err());
            assert_eq!(io.count, failure - 1);
            let count = io.count;
            let _ = lease.restore(&mut io);
            assert_eq!(io.count, count, "No input after journal failure");
        }
    }

    #[test]
    fn failed_close_checkpoint_cannot_take_no_change_fast_path() {
        let mut io = Model::new();
        for (k, v) in [
            ("LastActiveTool", "primary"),
            ("LastPen", "Finelinerv2"),
            ("LastFinelinerv2Color", "Black"),
            ("LastFinelinerv2Size", "2"),
        ] {
            io.prefs.insert(k.into(), v.into());
        }
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "reader-status-close-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        io.journal = Some(Journal::create(&path, &lease.recovery).unwrap());
        io.fail_checkpoint = Some(2);
        assert!(lease.acquire(&mut io).is_err());
        assert_eq!(lease.recovery.phase, Phase::CloseForDrawing);
        assert_eq!(io.count, 1);
        // Even a later closed-looking original toolbar cannot make the failed
        // checkpoint disappear through the unchanged-style fast path.
        io.menu = false;
        assert!(lease.restore(&mut io).is_err());
        assert_eq!(io.count, 1);
        assert!(path.exists());
        assert_eq!(Recovery::read(&path).unwrap().phase, Phase::OpenPrimary);
        drop(io.journal.take());
        fs::remove_file(path).unwrap();
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
    fn changed_native_frame_refuses_before_any_tool_mutation() {
        let mut io = Model::new();
        io.canvas = Some(fixture(include_bytes!(
            "../../tests/fixtures/status-style/native-page-overlay.png"
        )));
        let lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        io.canvas = Some(fixture(include_bytes!(
            "../../tests/fixtures/status-style/native-page-overlay-settled.png"
        )));
        let error = lease.observe(&mut io).err().unwrap().to_string();
        assert!(
            error.contains("Status page image changed at (586, 824)"),
            "{error}"
        );
        assert_eq!(io.count, 0);
        assert!(io.checkpoints.is_empty());
        assert_eq!(lease.recovery.phase, Phase::Prepared);
        // A newly observed settled frame is stable; do not weaken the old lease.
        let settled = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        assert!(settled.observe(&mut io).is_ok());
    }

    #[test]
    fn internal_native_footer_pair_preserves_strict_pre_mutation_refusal() {
        let before = fixture(include_bytes!(
            "../../tests/fixtures/status-style/native-footer-before.png"
        ));
        let after = fixture(include_bytes!(
            "../../tests/fixtures/status-style/native-footer-after.png"
        ));
        // Actual f38748d internal lease pair. Unlike the REM34 external frames,
        // this pair differs only in the observed bottom navigation overlay.
        let changed: Vec<_> = before
            .enumerate_pixels()
            .filter_map(|(x, y, p)| (p[0].abs_diff(after.get_pixel(x, y)[0]) > 8).then_some((x, y)))
            .collect();
        assert_eq!(changed.len(), 1414);
        assert_eq!(changed[0], (143, 991));
        assert!(changed
            .iter()
            .all(|(x, y)| (138..=629).contains(x) && (991..=1011).contains(y)));
        let mut io = Model::new();
        io.canvas = Some(before);
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        io.canvas = Some(after);
        assert!(lease
            .acquire(&mut io)
            .unwrap_err()
            .to_string()
            .contains("(143, 991)"));
        assert!(lease.restore(&mut io).is_err());
        assert_eq!(io.count, 0);
        assert!(io.checkpoints.is_empty());
        assert_eq!(lease.recovery.phase, Phase::Prepared);
    }

    #[test]
    fn changed_page_refuses_rollback_input() {
        let mut io = Model::new();
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
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
        assert!(Lease::prepare(state, false).unwrap().is_none());
        let mut state = io.observe().unwrap();
        // A docked popover elsewhere on the seam has no pen-menu grid at all.
        for y in 700..850 {
            state.image.put_pixel(61, y, Luma([0]));
        }
        assert!(Lease::prepare(state, false).unwrap().is_none());
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
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        lease.acquire(&mut io).unwrap();
        let count = io.count;
        lease.restore(&mut io).unwrap();
        assert_eq!(count, io.count);
        // Previously13. No input occurs between the consolidated property
        // decisions; transition/cleanup verification still uses fresh captures.
        assert_eq!(io.observations, 8);
        assert_eq!(io.prefs, original);
        assert!(!io.menu);
    }

    #[test]
    fn changed_temporary_style_is_rejected_before_closing_menu() {
        let mut io = Model::new();
        for (key, value) in [
            ("LastActiveTool", "primary"),
            ("LastPen", "Finelinerv2"),
            ("LastFinelinerv2Color", "Black"),
            ("LastFinelinerv2Size", "2"),
        ] {
            io.prefs.insert(key.into(), value.into());
        }
        let mut lease = Lease::prepare(io.observe().unwrap(), false)
            .unwrap()
            .unwrap();
        // First capture before closing, after the captured original settings.
        io.change_style_at_observation = Some(6);
        let error = lease.acquire(&mut io).unwrap_err();
        assert!(error.to_string().contains("Temporary style not verified"));
        assert_eq!(io.count, 1); // only opened the menu; no close or drawing
        assert!(io.menu);
        assert_eq!(lease.recovery.original_fine, Some((0, 1)));
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
    CurrentInk,
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
    PendingCleanup,
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
                | Self::PendingCleanup
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
            matches!(self.version, 2 | 3) && self.sequence < RECORD_COUNT,
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
        if self.version == 3 {
            ensure!(
                self.mutations == Mutations::default()
                    && self.original_grid.is_none()
                    && self.original_fine.is_none()
                    && matches!(
                        (self.sequence, self.phase),
                        (0, Phase::Prepared) | (1, Phase::CurrentInk) | (2, Phase::PendingCleanup)
                    ),
                "Invalid current-tool recovery intent"
            );
            return Ok(());
        }
        let intended = match self.phase {
            Phase::CurrentInk => false,
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
            Phase::PendingCleanup => self.original_grid.is_some() && self.original_fine.is_some(),
        };
        ensure!(intended, "Recovery phase lacks mutation intent");
        Ok(())
    }
    fn follows(&self, old: &Self) -> Result<()> {
        self.validate()?;
        ensure!(
            old.phase != Phase::PendingCleanup,
            "Cleanup checkpoint is terminal"
        );
        ensure!(
            self.version == old.version
                && self.sequence == old.sequence + 1
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

/// Positive closed-toolbar candidate only; this does not establish the unknown
/// width or an eraser envelope. Preferences are deliberately not an input.
fn closed_black_fineliner(image: &GrayImage) -> Option<&'static str> {
    static REFERENCE: std::sync::LazyLock<GrayImage> = std::sync::LazyLock::new(|| {
        image::load_from_memory(include_bytes!("fixtures/current-black-fineliner.png"))
            .expect("checked native Fineliner fixture")
            .to_luma8()
    });
    static SECONDARY: std::sync::LazyLock<GrayImage> = std::sync::LazyLock::new(|| {
        image::load_from_memory(include_bytes!(
            "fixtures/current-secondary-black-fineliner.png"
        ))
        .expect("checked native secondary Fineliner fixture")
        .to_luma8()
    });
    let ui = controls(image)?;
    if ui.menu {
        return None;
    }
    let offset = if ui.slot == "secondary" { 62 } else { 0 };
    let reference = if ui.slot == "secondary" {
        &*SECONDARY
    } else {
        &*REFERENCE
    };
    (61..123)
        .all(|y| {
            (0..61).all(|x| {
                image.get_pixel(x, y + offset).0[0].abs_diff(reference.get_pixel(x, y - 61).0[0])
                    <= 8
            })
        })
        .then_some(ui.slot)
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
    fn monotonic(&self) -> Duration;
    fn pace(&mut self, duration: Duration);
    /// Observe external input throughout the no-input post-press wait. Native
    /// touch injection shares the physical device, so this starts after release.
    fn begin_wait(&mut self) -> Result<()>;
    fn check_wait(&mut self) -> Result<()>;
    fn end_wait(&mut self);
}

pub struct Lease {
    debug_dump: bool,
    pub recovery: Recovery,
    baseline: GrayImage,
    checkpoint_failed: bool,
    viewport: CleanupViewport,
    before_cleanup: Option<GrayImage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanupViewport {
    Blank,
    Landmarks,
}
impl CleanupViewport {
    fn classify(image: &GrayImage) -> Option<Self> {
        if (61..1024).all(|y| (61..768).all(|x| image.get_pixel(x, y).0[0] >= 248)) {
            return Some(Self::Blank);
        }
        let landmarks = [(96, 128), (416, 128), (96, 576)]
            .into_iter()
            .all(|(x, y)| {
                let mut rows = [false; 4];
                let mut cols = [false; 4];
                let mut cells = 0;
                for row in 0..4 {
                    for col in 0..4 {
                        let mut dark = 0;
                        let mut light = 0;
                        for yy in y + row * 64..y + (row + 1) * 64 {
                            for xx in x + col * 64..x + (col + 1) * 64 {
                                let value = image.get_pixel(xx, yy).0[0];
                                dark += usize::from(value < 200);
                                light += usize::from(value >= 248);
                            }
                        }
                        if dark >= 16 && light >= 128 {
                            cells += 1;
                            rows[row as usize] = true;
                            cols[col as usize] = true;
                        }
                    }
                }
                cells >= 6
                    && rows.into_iter().filter(|v| *v).count() >= 3
                    && cols.into_iter().filter(|v| *v).count() >= 3
            });
        landmarks.then_some(Self::Landmarks)
    }
    fn unchanged(self, before: &GrayImage, after: &GrayImage) -> bool {
        before.dimensions() == after.dimensions()
            && (0..1024).all(|y| {
                (61..768).all(|x| {
                    // Only landmark-qualified pages can tolerate the empirically observed
                    // native redraw region. This is viewport evidence, not ink proof.
                    (self == Self::Landmarks && x >= 576 && y >= 800)
                        || before.get_pixel(x, y).0[0].abs_diff(after.get_pixel(x, y).0[0]) <= 8
                })
            })
    }
}
impl Lease {
    /// Candidate current-tool path. Callers must separately establish the native
    /// footprint contract before enabling this in the product. No menu input.
    pub fn prepare_current(observed: Observation, debug_dump: bool) -> Result<Option<Self>> {
        if closed_black_fineliner(&observed.image).is_none() {
            return Ok(None);
        }
        let Some(mut lease) = Self::prepare(observed, debug_dump)? else {
            return Ok(None);
        };
        lease.recovery.version = 3;
        lease.recovery.validate()?;
        Ok(Some(lease))
    }
    pub fn prepare(observed: Observation, debug_dump: bool) -> Result<Option<Self>> {
        let Some(ui) = controls(&observed.image) else {
            return Ok(None);
        };
        if ui.menu {
            return Ok(None);
        }
        let Some(viewport) = CleanupViewport::classify(&observed.image) else {
            return Ok(None);
        };
        let recovery = Recovery::new(observed.identity, observed.preferences, ui.slot.to_owned())?;
        Ok(Some(Self {
            recovery,
            debug_dump,
            baseline: observed.image,
            checkpoint_failed: false,
            viewport,
            before_cleanup: None,
        }))
    }
    fn observe(&self, io: &mut impl StyleIo) -> Result<(Observation, Controls)> {
        let started = self.debug_dump.then(|| io.monotonic());
        let state = io.observe()?;
        let returned = self.debug_dump.then(|| io.monotonic());
        ensure!(
            state.identity == self.recovery.identity,
            "Status page/session changed; restoration stopped"
        );
        ensure!(
            state.image.dimensions() == self.baseline.dimensions(),
            "Status image dimensions changed"
        );
        let changed = (0..1024).find_map(|y| {
            (0..768).find_map(|x| {
                // Native PDF calibration: only the undo icon enabled here.
                let unchanged = (x < 61 && self.recovery.version == 2)
                    || (self.recovery.version == 3
                        && (20..=40).contains(&x)
                        && (391..=405).contains(&y))
                    || (self.recovery.version == 2 && x < 280 && (61..651).contains(&y))
                    || (x >= 686
                        && y >= 922
                        && (self.recovery.version == 2 || (x <= 759 && y <= 995)))
                    || state.image.get_pixel(x, y).0[0]
                        .abs_diff(self.baseline.get_pixel(x, y).0[0])
                        <= 8;
                (!unchanged).then_some((x, y))
            })
        });
        if let Some((x, y)) = changed {
            if self.debug_dump {
                // Fixed, bounded opt-in evidence; never replace the model capture.
                for (name, image) in [("before", &self.baseline), ("rejected", &state.image)] {
                    if let Err(error) =
                        image.save(format!("/tmp/reader-buddy-status-lease-{name}.png"))
                    {
                        log::warn!("Could not save status diagnostic: {error}");
                    }
                }
            }
            anyhow::bail!("Status page image changed at ({x}, {y}); restoration stopped");
        }
        let Some(ui) = controls(&state.image) else {
            if let (Some(started), Some(returned)) = (started, returned) {
                if let Err(error) = self.dump_controls_refusal(&state, started, returned) {
                    log::warn!("Could not save toolbar refusal diagnostic: {error:#}");
                }
            }
            anyhow::bail!(
                "Status toolbar layout changed (last intent {:?}, sequence {})",
                self.recovery.phase,
                self.recovery.sequence
            );
        };
        Ok((state, ui))
    }
    fn dump_controls_refusal(
        &self,
        state: &Observation,
        started: Duration,
        returned: Duration,
    ) -> Result<()> {
        let stem = format!(
            "/tmp/reader-buddy-toolbar-refusal-{}-{}",
            std::process::id(),
            self.recovery.sequence
        );
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut image_file = match options.open(format!("{stem}.png")) {
            Ok(file) => file,
            // Preserve the first actual rejected frame for this durable intent.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        state
            .image
            .write_to(&mut image_file, image::ImageFormat::Png)?;
        let expected = match self.recovery.phase {
            Phase::SelectPrimary
            | Phase::RestoreSelectPrimary
            | Phase::CloseForDrawing
            | Phase::RestoreClose => serde_json::json!({"slot":"primary","menu":false}),
            Phase::OpenPrimary | Phase::RestoreOpen => {
                serde_json::json!({"slot":"primary","menu":true})
            }
            Phase::SelectFine => serde_json::json!({"slot":"primary","menu":true,"grid":1}),
            Phase::SetColor => serde_json::json!({"grid":1,"color_index":0}),
            Phase::SetWidth => serde_json::json!({"grid":1,"width_index":1}),
            Phase::RestoreColor => {
                serde_json::json!({"grid":1,"color_index":self.recovery.original_fine.map(|f| f.0)})
            }
            Phase::RestoreWidth => {
                serde_json::json!({"grid":1,"width_index":self.recovery.original_fine.map(|f| f.1)})
            }
            Phase::RestorePrimary => {
                serde_json::json!({"slot":"primary","menu":true,"grid":self.recovery.original_grid})
            }
            Phase::RestoreSlot | Phase::Prepared | Phase::CurrentInk | Phase::PendingCleanup => {
                serde_json::json!({"slot":self.recovery.original_slot,"menu":false})
            }
        };
        let metadata = serde_json::json!({
            "meaning": "Actual rejected observation after identity/content checks; last intent is not proof of input completion",
            "observer_pid": std::process::id(),
            "clock": "StyleIo monotonic elapsed time; not a wall-clock timestamp",
            "identity": state.identity,
            "sequence": self.recovery.sequence,
            "last_intent": self.recovery.phase,
            "expected_after_last_intent": expected,
            "original_grid": self.recovery.original_grid,
            "original_fine": self.recovery.original_fine,
            "observation_started_us": started.as_micros(),
            "observation_returned_us": returned.as_micros(),
        });
        options
            .open(format!("{stem}.json"))?
            .write_all(&serde_json::to_vec_pretty(&metadata)?)?;
        Ok(())
    }
    fn transition(
        &mut self,
        io: &mut impl StyleIo,
        phase: Phase,
        point: (u32, u32),
        expected: impl Fn(&Controls) -> bool,
    ) -> Result<()> {
        ensure!(
            self.recovery.version == 2,
            "Current-tool lease forbids toolbar input"
        );
        let (_, before) = self.observe(io)?;
        if phase == Phase::CloseForDrawing {
            ensure!(
                Self::fine_controls(&before)? == (0, 1),
                "Temporary style not verified before closing"
            );
        }
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
            Phase::Prepared | Phase::CurrentInk | Phase::PendingCleanup => {
                anyhow::bail!("Phase cannot send toolbar input")
            }
        }
        self.recovery.phase = phase;
        self.recovery.sequence += 1;
        // Record intent durably BEFORE input: a failed press may have acted.
        if let Err(error) = io.checkpoint(&self.recovery) {
            self.checkpoint_failed = true;
            return Err(error);
        }
        io.press(point)?;
        let deadline = io.monotonic().saturating_add(Duration::from_secs(5));
        io.begin_wait()?;
        let result = (|| {
            loop {
                io.check_wait()?;
                ensure!(
                    io.monotonic() < deadline,
                    "Status controls did not converge before deadline"
                );
                let (_, ui) = self.observe(io)?;
                io.check_wait()?;
                // A slow capture cannot turn an expired operation into success.
                ensure!(
                    io.monotonic() < deadline,
                    "Status observation exceeded deadline"
                );
                if expected(&ui) {
                    return Ok(());
                }
                io.pace(Duration::from_millis(50).min(deadline.saturating_sub(io.monotonic())));
            }
        })();
        io.end_wait();
        result
    }
    fn primary_menu(&mut self, io: &mut impl StyleIo, restoring: bool) -> Result<()> {
        let (_, mut ui) = self.observe(io)?;
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
            // The slot changed: never reuse the pre-input observation.
            ui = self.observe(io)?.1;
        }
        if !ui.menu {
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
        Self::fine_controls(&ui)
    }
    fn fine_controls(ui: &Controls) -> Result<(usize, usize)> {
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
        if self.recovery.version == 3 {
            ensure!(
                self.recovery.phase == Phase::Prepared,
                "Duplicate current-tool acquisition"
            );
            io.begin_wait()?;
            let result = (|| {
                io.check_wait()?;
                self.observe_current(io)?;
                self.recovery.phase = Phase::CurrentInk;
                self.recovery.sequence = 1;
                if let Err(error) = io.checkpoint(&self.recovery) {
                    self.checkpoint_failed = true;
                    return Err(error);
                }
                self.observe_current(io)?;
                io.check_wait()
            })();
            io.end_wait();
            return result;
        }
        self.primary_menu(io, false)?;
        let (_, mut ui) = self.observe(io)?;
        self.recovery.original_grid = Some(ui.grid.context("Missing actual primary tool grid")?);
        if ui.grid != Some(1) {
            self.transition(io, Phase::SelectFine, GRID[1], |u| u.grid == Some(1))?;
            ui = self.observe(io)?.1;
        }
        let mut current = Self::fine_controls(&ui)?;
        self.recovery.original_fine = Some(current);
        // These are decisions from the same fresh menu, without intervening
        // device input, callback or checkpoint. A mutation requires a new read.
        if current.0 != 0 {
            self.transition(io, Phase::SetColor, PALETTE[0], |u| {
                u.fine.is_some_and(|f| f.0 == 0)
            })?;
            current = self.known_fine(io)?;
        }
        if current.1 != 1 {
            self.transition(io, Phase::SetWidth, WIDTHS[1], |u| {
                u.fine.is_some_and(|f| f.1 == 1)
            })?;
        }
        // transition checks the exact temporary style in its fresh pre-input
        // observation, then verifies the closed primary slot after the press.
        self.transition(io, Phase::CloseForDrawing, (30, 90), |u| {
            !u.menu && u.slot == "primary"
        })
    }
    pub fn restore(&mut self, io: &mut impl StyleIo) -> Result<()> {
        ensure!(
            !self.cleanup_pending(),
            "Cleanup pending; toolbar input forbidden"
        );
        ensure!(
            !self.checkpoint_failed,
            "Recovery checkpoint failed; retain evidence and stop input"
        );
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
    /// Fresh ownership/content/tool proof; does not mutate or infer saved width.
    pub fn verify_current(&self, io: &mut impl StyleIo) -> Result<()> {
        ensure!(
            self.recovery.phase == Phase::CurrentInk,
            "No active owned ink intent"
        );
        self.observe_current(io)
    }
    fn observe_current(&self, io: &mut impl StyleIo) -> Result<()> {
        ensure!(
            self.recovery.version == 3 && !self.checkpoint_failed,
            "No valid current-tool lease"
        );
        let (state, ui) = self.observe(io)?;
        ensure!(
            !ui.menu
                && ui.slot == self.recovery.original_slot
                && closed_black_fineliner(&state.image) == Some(ui.slot)
                && self.original_controls(&state.image),
            "Current drawing tool changed"
        );
        Ok(())
    }
    pub fn cleanup_pending(&self) -> bool {
        self.recovery.phase == Phase::PendingCleanup
    }
    pub fn prepare_cleanup(&mut self, io: &mut impl StyleIo) -> Result<()> {
        if self.recovery.version == 3 {
            io.begin_wait()?;
            let result = (|| {
                io.check_wait()?;
                ensure!(
                    self.recovery.phase == Phase::CurrentInk,
                    "No owned ink intent"
                );
                self.prepare_cleanup_inner(io)?;
                io.check_wait()
            })();
            io.end_wait();
            return result;
        }
        self.prepare_cleanup_inner(io)
    }
    fn prepare_cleanup_inner(&mut self, io: &mut impl StyleIo) -> Result<()> {
        self.restore(io)?;
        let (state, ui) = self.observe(io)?;
        ensure!(
            !ui.menu
                && ui.slot == self.recovery.original_slot
                && self.original_controls(&state.image),
            "Original controls changed before cleanup"
        );
        self.recovery.phase = Phase::PendingCleanup;
        self.recovery.sequence += 1;
        if let Err(error) = io.checkpoint(&self.recovery) {
            self.checkpoint_failed = true;
            return Err(error);
        }
        // Bracket the durable checkpoint with fresh strict observations. No
        // erasure is permitted if page, controls, session or viewport changed.
        let (state, ui) = self.observe(io)?;
        ensure!(
            !ui.menu
                && ui.slot == self.recovery.original_slot
                && self.original_controls(&state.image),
            "Original controls changed before erasure"
        );
        self.before_cleanup = Some(state.image);
        Ok(())
    }
    pub fn finish_cleanup(&self, io: &mut impl StyleIo) -> Result<()> {
        if self.recovery.version == 3 {
            io.begin_wait()?;
            let result = (|| {
                io.check_wait()?;
                self.finish_cleanup_inner(io)?;
                io.check_wait()
            })();
            io.end_wait();
            return result;
        }
        self.finish_cleanup_inner(io)
    }
    fn finish_cleanup_inner(&self, io: &mut impl StyleIo) -> Result<()> {
        ensure!(
            self.cleanup_pending() && !self.checkpoint_failed,
            "No valid pending cleanup checkpoint"
        );
        let before = self
            .before_cleanup
            .as_ref()
            .context("Cleanup was not prepared")?;
        let state = io.observe()?;
        ensure!(
            state.identity == self.recovery.identity,
            "Cleanup page/session changed"
        );
        ensure!(
            state.image.dimensions() == self.baseline.dimensions(),
            "Cleanup dimensions changed"
        );
        let ui = controls(&state.image).context("Cleanup toolbar layout changed")?;
        ensure!(
            !ui.menu
                && ui.slot == self.recovery.original_slot
                && self.original_controls(&state.image),
            "Cleanup original controls changed"
        );
        let reference = if self.viewport == CleanupViewport::Blank {
            &self.baseline
        } else {
            before
        };
        ensure!(
            self.viewport.unchanged(reference, &state.image),
            "Cleanup viewport changed"
        );
        if self.recovery.version == 3 {
            use crate::workflow::indicator::{BOTTOM, LEFT, RIGHT, TOP};
            ensure!(
                (0..1024).all(|y| (0..61).all(|x| {
                    ((20..=40).contains(&x) && (391..=405).contains(&y))
                        || before.get_pixel(x, y).0[0].abs_diff(state.image.get_pixel(x, y).0[0])
                            <= 8
                })),
                "Cleanup toolbar changed outside observed undo icon"
            );
            // The inherited landmark redraw allowance must never hide changed
            // neighbors of the erased footprint. Require an additional ring
            // outside the existing 12-pixel blank clearance to remain exact.
            ensure!(
                (TOP - 24..=(BOTTOM + 24).min(1023)).all(|y| {
                    (LEFT - 24..=(RIGHT + 24).min(767)).all(|x| {
                        ((LEFT - 12..=RIGHT + 12).contains(&x)
                            && (TOP - 12..=BOTTOM + 12).contains(&y))
                            || before.get_pixel(x as u32, y as u32).0[0]
                                .abs_diff(state.image.get_pixel(x as u32, y as u32).0[0])
                                <= 8
                    })
                }),
                "Cleanup changed ink beside the reserved footprint"
            );
        }
        ensure!(
            crate::workflow::indicator::eligible(&image::DynamicImage::ImageLuma8(state.image)),
            "Cleanup corner is not clear"
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
