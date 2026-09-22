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
const COLORS: [&str; 9] = [
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
        menu: bool,
        count: usize,
        fail: Option<usize>,
        identity: Identity,
    }
    impl Model {
        fn new() -> Self {
            Self {
                prefs: prefs(),
                menu: false,
                count: 0,
                fail: None,
                identity: identity(),
            }
        }
    }
    impl StyleIo for Model {
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
                    let (color, width) = fine_style(&self.prefs)?;
                    box_at(&mut image, PALETTE[color]);
                    box_at(&mut image, WIDTHS[width]);
                }
            }
            Ok(Observation {
                identity: self.identity.clone(),
                preferences: self.prefs.clone(),
                image,
            })
        }
        fn press(&mut self, p: (u32, u32)) -> Result<()> {
            self.count += 1;
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
        for failure in 1..=6 {
            let mut io = Model::new();
            io.fail = Some(failure);
            let mut lease = Lease::prepare(io.observe().unwrap()).unwrap().unwrap();
            assert!(lease.acquire(&mut io).is_err(), "failure {failure}");
            lease.restore(&mut io).unwrap();
            assert_eq!(io.prefs, prefs(), "failure {failure}");
            assert!(!io.menu);
        }
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
        let record = Recovery::new(identity(), prefs()).unwrap();
        record.create(&path).unwrap();
        assert!(record.create(&path).is_err());
        assert_eq!(Recovery::read(&path).unwrap().preferences, prefs());
        fs::write(&path, b"{\"version\":9}").unwrap();
        assert!(Recovery::read(&path).is_err());
        assert!(record.create(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    version: u32,
    pub identity: Identity,
    pub preferences: Preferences,
}

impl Recovery {
    pub fn new(identity: Identity, preferences: Preferences) -> Result<Self> {
        let record = Self {
            version: 1,
            identity,
            preferences,
        };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported status recovery version");
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
            self.preferences.len() <= 128
                && self
                    .preferences
                    .iter()
                    .all(|(k, v)| k.len() <= 80 && v.len() <= 80),
            "Status preferences exceed bounds"
        );
        ensure!(
            matches!(
                self.preferences.get("LastActiveTool").map(String::as_str),
                Some("primary" | "secondary")
            ),
            "Unsupported active tool"
        );
        ensure!(
            self.preferences
                .get("LastPen")
                .is_some_and(|v| !v.is_empty()),
            "Missing primary tool"
        );
        fine_style(&self.preferences)?;
        Ok(())
    }

    /// create_new provides the exclusive claim. A crash/partial write leaves an
    /// unresolved file, which is a refusal, never permission to overwrite it.
    pub fn create(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        ensure!(bytes.len() <= 32768, "Status recovery exceeds bounds");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .context("Unresolved status recovery record or unavailable cache")?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        fs::File::open(path)?.take(32769).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 32768, "Status recovery exceeds bounds");
        let record: Self = serde_json::from_slice(&bytes)?;
        record.validate()?;
        Ok(record)
    }
}

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
    fn press(&mut self, point: (u32, u32)) -> Result<()>;
}

pub struct Lease {
    pub recovery: Recovery,
    baseline: GrayImage,
    original_grid: Option<usize>,
    mutation_intended: bool,
}

impl Lease {
    pub fn prepare(observed: Observation) -> Result<Option<Self>> {
        let Some(ui) = controls(&observed.image) else {
            return Ok(None);
        };
        if ui.menu
            || observed
                .preferences
                .get("LastActiveTool")
                .map(String::as_str)
                != Some(ui.slot)
        {
            return Ok(None);
        }
        let Ok(recovery) = Recovery::new(observed.identity, observed.preferences) else {
            return Ok(None);
        };
        Ok(Some(Self {
            recovery,
            baseline: observed.image,
            original_grid: None,
            mutation_intended: false,
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
        let unchanged = |prefs: &Preferences| {
            prefs
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "LastPen"
                            | "LastActiveTool"
                            | "LastFinelinerv2Color"
                            | "LastFinelinerv2Size"
                    )
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Preferences>()
        };
        ensure!(
            unchanged(&state.preferences) == unchanged(&self.recovery.preferences),
            "Other tool preferences changed; status input stopped"
        );
        // Exclude only toolbar/menu and owned status area. Compare all remaining
        // page pixels, including the left margin below the menu.
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
        point: (u32, u32),
        expected: impl Fn(&Observation, &Controls) -> bool,
    ) -> Result<()> {
        self.observe(io)?;
        // A failed input call may already have changed xochitl.
        self.mutation_intended = true;
        io.press(point)?;
        for _ in 0..5 {
            let (state, ui) = self.observe(io)?;
            if expected(&state, &ui) {
                return Ok(());
            }
        }
        anyhow::bail!("Status controls/preferences did not converge after input")
    }

    fn primary_menu(&mut self, io: &mut impl StyleIo) -> Result<()> {
        let (_, ui) = self.observe(io)?;
        if ui.slot != "primary" {
            ensure!(!ui.menu, "Unexpected secondary menu");
            self.transition(io, (30, 90), |s, u| {
                u.slot == "primary"
                    && !u.menu
                    && s.preferences
                        .get("LastActiveTool")
                        .is_some_and(|s| s == "primary")
            })?;
        }
        if !self.observe(io)?.1.menu {
            self.transition(io, (30, 90), |_, u| u.slot == "primary" && u.menu)?;
        }
        Ok(())
    }

    fn set_fine(&mut self, io: &mut impl StyleIo, desired: (usize, usize)) -> Result<()> {
        let (state, ui) = self.observe(io)?;
        ensure!(
            ui.slot == "primary"
                && ui.grid == Some(1)
                && ui.fine == Some(fine_style(&state.preferences)?),
            "Fineliner menu/preferences disagree"
        );
        if ui.fine.unwrap().0 != desired.0 {
            self.transition(io, PALETTE[desired.0], |s, u| {
                u.fine.is_some_and(|f| f.0 == desired.0)
                    && fine_style(&s.preferences).is_ok_and(|f| f.0 == desired.0)
            })?;
        }
        if self.observe(io)?.1.fine.context("Fineliner menu lost")?.1 != desired.1 {
            self.transition(io, WIDTHS[desired.1], |s, u| {
                u.fine.is_some_and(|f| f.1 == desired.1)
                    && fine_style(&s.preferences).is_ok_and(|f| f.1 == desired.1)
            })?;
        }
        Ok(())
    }

    pub fn acquire(&mut self, io: &mut impl StyleIo) -> Result<()> {
        self.primary_menu(io)?;
        let (state, ui) = self.observe(io)?;
        ensure!(
            state.preferences.get("LastPen") == self.recovery.preferences.get("LastPen"),
            "Primary tool changed during acquisition"
        );
        self.original_grid = ui.grid;
        if ui.grid != Some(1) {
            self.transition(io, GRID[1], |s, u| {
                u.grid == Some(1)
                    && u.fine.is_some()
                    && s.preferences
                        .get("LastPen")
                        .is_some_and(|s| s == "Finelinerv2")
            })?;
        }
        // Medium Fineliner produced distinct native geometry in4105ff3. This is
        // a narrow footprint relative to Highlighter, not a claim about all pens.
        self.set_fine(io, (0, 1))?;
        self.transition(io, (30, 90), |_, u| !u.menu && u.slot == "primary")
    }

    pub fn restore(&mut self, io: &mut impl StyleIo) -> Result<()> {
        if !self.mutation_intended {
            return Ok(());
        }
        let (state, ui) = self.observe(io)?;
        // Acquisition only opened/closed our menu when the original primary
        // Fineliner was already black medium. A fresh full observation verifies
        // the unchanged settings without opening the menu a second time.
        if self.original_grid == Some(1)
            && self.recovery.preferences["LastActiveTool"] == "primary"
            && fine_style(&self.recovery.preferences)? == (0, 1)
            && !ui.menu
            && ui.slot == "primary"
            && state.preferences == self.recovery.preferences
        {
            return Ok(());
        }
        if let Some(original) = self.original_grid {
            self.primary_menu(io)?;
            let ui = self.observe(io)?.1;
            if ui.grid == Some(1) {
                self.set_fine(io, fine_style(&self.recovery.preferences)?)?;
            } else {
                ensure!(
                    fine_style(&state.preferences)? == fine_style(&self.recovery.preferences)?,
                    "Cannot restore changed Fineliner from an unexpected tool"
                );
            }
            if self.observe(io)?.1.grid != Some(original) {
                let tool = self.recovery.preferences["LastPen"].clone();
                self.transition(io, GRID[original], |s, u| {
                    u.grid == Some(original) && s.preferences.get("LastPen") == Some(&tool)
                })?;
            }
        }
        let ui = self.observe(io)?.1;
        if ui.menu {
            ensure!(ui.slot == "primary", "Refuse to close unowned menu");
            self.transition(io, (30, 90), |_, u| !u.menu)?;
        }
        if self.recovery.preferences["LastActiveTool"] == "secondary"
            && self.observe(io)?.1.slot != "secondary"
        {
            self.transition(io, (30, 150), |s, u| {
                u.slot == "secondary"
                    && !u.menu
                    && s.preferences
                        .get("LastActiveTool")
                        .is_some_and(|s| s == "secondary")
            })?;
        }
        let (state, ui) = self.observe(io)?;
        ensure!(
            !ui.menu && state.preferences == self.recovery.preferences,
            "Exact status tool preferences were not restored"
        );
        Ok(())
    }
}
