//! Serialized native history adapter. No page replacement or compensating undo.
use super::{
    input_observer::InputObserver,
    interaction::Interaction,
    keyboard::Keyboard,
    native_page,
    touch::{Touch, TriggerCorner},
    DeviceModel,
};
use crate::workflow::history::{Command, Owner, PageState};
use anyhow::{ensure, Context, Result};
use std::{
    path::Path,
    thread::sleep,
    time::{Duration, Instant},
};

const ROOT: &str = "/home/root/.local/share/remarkable/xochitl";
const SETTINGS: &str = "/home/root/.config/remarkable/xochitl.conf";
const HEADER: &str = "=== Reader Buddy Answers ===\n\n\n";

pub struct NativeHistory {
    input: Option<InputObserver>,
    owner: Option<Owner>,
    entry: bool,
    pending_header: Option<String>,
    corner: TriggerCorner,
    supported: bool,
}

impl NativeHistory {
    pub fn new(corner: TriggerCorner) -> Self {
        let supported = std::fs::read_to_string("/etc/os-release")
            .is_ok_and(|release| native_page::verified_contract(DeviceModel::detect(), &release));
        if !supported {
            log::info!("Native Q&A history unavailable for this device/firmware contract");
        }
        Self {
            input: None,
            owner: None,
            entry: false,
            pending_header: None,
            corner,
            supported,
        }
    }

    pub fn discard(&mut self) {
        self.owner = None;
        self.entry = false;
        // Keep the contact reducer alive: invalidation at250ms must not restart
        // the same single-finger Reader hold's two-second timer.
    }

    pub fn note_render(&mut self, text: &str) {
        self.pending_header = (text == HEADER).then(|| text.to_owned());
    }
    pub fn other_edit(&mut self) {
        self.pending_header = None;
    }

    fn new_input(&self, keyboard: &mut Keyboard) -> Result<InputObserver> {
        let owned = keyboard
            .owned_sysfs()?
            .context("History requires an owned keyboard")?;
        InputObserver::new(self.corner.clone(), Some(&owned))
    }

    fn input_guard(&mut self) -> Result<()> {
        let events = self
            .input
            .as_mut()
            .context("History input observer unavailable")?
            .poll()?;
        ensure!(
            events.is_empty() && self.input.as_ref().is_some_and(InputObserver::quiescent),
            "External input interrupted Q&A history ownership"
        );
        Ok(())
    }

    fn owner_guard(&mut self, owner: &Owner) -> Result<()> {
        self.input_guard()?;
        ensure!(
            native_page::session_matches(Path::new("/proc"), &owner.session)?,
            "Native session changed"
        );
        ensure!(
            native_page::observed_owner(
                Path::new(ROOT),
                Path::new(SETTINGS),
                owner.session.clone()
            )? == *owner,
            "Native page/visit changed"
        );
        Ok(())
    }

    fn settled(
        &mut self,
        expected: Option<&str>,
        required_owner: Option<&Owner>,
    ) -> Result<PageState> {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(30);
        let mut previous = None;
        let mut repeats = 0;
        let mut last_error = "Native state did not settle".to_owned();
        loop {
            self.input_guard()?;
            if let Some(owner) = required_owner {
                self.owner_guard(owner)?;
            }
            let observed = (|| -> Result<PageState> {
                let session = native_page::xochitl_session(Path::new("/proc"))?;
                let page = native_page::observed_candidate(
                    Path::new(ROOT),
                    Path::new(SETTINGS),
                    session.clone(),
                )?;
                ensure!(
                    native_page::session_matches(Path::new("/proc"), &session)?,
                    "Native session changed"
                );
                ensure!(
                    expected.is_none_or(|text| page.content.text() == text),
                    "Persisted native text has not reached expected complete content"
                );
                ensure!(
                    required_owner.is_none_or(|owner| page.owner == *owner),
                    "Native page/visit changed"
                );
                Ok(page)
            })();
            self.input_guard()?;
            match observed {
                Ok(mut page) => {
                    if previous.as_ref() == Some(&page) {
                        repeats += 1;
                    } else {
                        repeats = 0;
                    }
                    if repeats >= 2 {
                        page.supported = true;
                        log::debug!(
                            "Native history content settled after {}ms",
                            started.elapsed().as_millis()
                        );
                        return Ok(page);
                    }
                    previous = Some(page);
                }
                Err(error) => {
                    previous = None;
                    repeats = 0;
                    last_error = error.to_string();
                }
            }
            ensure!(Instant::now() < deadline, "{last_error}");
            sleep(Duration::from_millis(100));
        }
    }

    pub fn snapshot(
        &mut self,
        keyboard: &mut Keyboard,
        expected: Option<&str>,
    ) -> Result<Option<PageState>> {
        if !self.supported {
            return Ok(None);
        }
        if self.owner.is_none() {
            ensure!(expected.is_none(), "History preparation is missing");
            self.input = Some(self.new_input(keyboard)?);
            let header = self.pending_header.take();
            let before = self.settled(header.as_deref(), None)?;
            self.owner = Some(before.owner.clone());
            Ok(Some(before))
        } else {
            let owner = self.owner.clone().unwrap();
            self.settled(expected, Some(&owner)).map(Some)
        }
    }

    pub fn mutate(
        &mut self,
        keyboard: &mut Keyboard,
        command: Command,
        expected: &str,
    ) -> Result<PageState> {
        let result = (|| -> Result<PageState> {
            ensure!(self.supported, "Native history contract unavailable");
            let owner = self.owner.clone().context("No native history owner")?;
            match command {
                Command::DeleteSuffix { .. } => {
                    ensure!(!self.entry, "Native deletion entry already exists")
                }
                _ => ensure!(self.entry, "No verified owned native deletion entry"),
            }
            self.owner_guard(&owner)?;
            keyboard.history_command(command, &mut || self.owner_guard(&owner))?;
            let result = self.settled(Some(expected), Some(&owner))?;
            self.entry = true;
            Ok(result)
        })();
        if result.is_err() {
            self.discard();
        }
        result
    }

    pub fn wait(
        &mut self,
        keyboard: &mut Keyboard,
        touch: &mut Touch,
        timeout: Option<Duration>,
    ) -> Result<Vec<Interaction>> {
        let deadline = timeout.map(|duration| Instant::now() + duration);
        if !self.supported {
            ensure!(timeout.is_none(), "Bounded input observation unavailable");
            touch.wait_for_trigger()?;
            return Ok(vec![Interaction::Reader]);
        }
        if self.input.is_none() {
            match self.new_input(keyboard) {
                Ok(input) => self.input = Some(input),
                Err(error) => {
                    log::warn!("History observer unavailable; retaining Reader trigger: {error}");
                    self.discard();
                    self.supported = false;
                    ensure!(timeout.is_none(), "Bounded input observation unavailable");
                    touch.wait_for_trigger()?;
                    return Ok(vec![Interaction::Invalidated, Interaction::Reader]);
                }
            }
        }
        loop {
            ensure!(
                deadline.is_none_or(|limit| Instant::now() < limit),
                "Input observation deadline reached"
            );
            match self.input.as_mut().unwrap().poll() {
                Ok(events) if !events.is_empty() => return Ok(events),
                Ok(_) => sleep(Duration::from_millis(10)),
                Err(error) => {
                    log::warn!("Q&A input ownership lost: {error}");
                    self.discard();
                    self.input = None;
                    return Ok(vec![Interaction::Invalidated]);
                }
            }
        }
    }
}
