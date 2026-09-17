use anyhow::Result;

#[cfg(target_os = "linux")]
use std::collections::HashMap;

#[cfg(target_os = "linux")]
use std::{thread, time};

#[cfg(target_os = "linux")]
use log::debug;

#[cfg(target_os = "linux")]
use evdev::{
    uinput::VirtualDevice, AttributeSet, EventType as EvdevEventType, InputEvent,
    KeyCode as EvdevKey,
};

#[cfg(target_os = "linux")]
pub struct Keyboard {
    device: Option<evdev::uinput::VirtualDevice>,
    rm2_keyboard: bool,
    key_map: HashMap<char, (EvdevKey, bool)>,
    progress_count: u32,
    no_draw_progress: bool,
}

#[cfg(not(target_os = "linux"))]
pub struct Keyboard {
    progress_count: u32,
    no_draw_progress: bool,
}

#[cfg(target_os = "linux")]
impl Keyboard {
    pub fn new(no_draw: bool, no_draw_progress: bool) -> Self {
        let device = if no_draw {
            None
        } else {
            Some(Self::create_virtual_device())
        };

        let rm2_keyboard = matches!(
            super::DeviceModel::detect(),
            super::DeviceModel::Remarkable2
        );
        Self {
            device,
            rm2_keyboard,
            key_map: Self::create_key_map(rm2_keyboard),
            progress_count: 0,
            no_draw_progress,
        }
    }

    fn create_virtual_device() -> evdev::uinput::VirtualDevice {
        debug!("Creating virtual keyboard");
        let mut keys = AttributeSet::<EvdevKey>::new();

        keys.insert(EvdevKey::KEY_A);
        keys.insert(EvdevKey::KEY_B);
        keys.insert(EvdevKey::KEY_C);
        keys.insert(EvdevKey::KEY_D);
        keys.insert(EvdevKey::KEY_E);
        keys.insert(EvdevKey::KEY_F);
        keys.insert(EvdevKey::KEY_G);
        keys.insert(EvdevKey::KEY_H);
        keys.insert(EvdevKey::KEY_I);
        keys.insert(EvdevKey::KEY_J);
        keys.insert(EvdevKey::KEY_K);
        keys.insert(EvdevKey::KEY_L);
        keys.insert(EvdevKey::KEY_M);
        keys.insert(EvdevKey::KEY_N);
        keys.insert(EvdevKey::KEY_O);
        keys.insert(EvdevKey::KEY_P);
        keys.insert(EvdevKey::KEY_Q);
        keys.insert(EvdevKey::KEY_R);
        keys.insert(EvdevKey::KEY_S);
        keys.insert(EvdevKey::KEY_T);
        keys.insert(EvdevKey::KEY_U);
        keys.insert(EvdevKey::KEY_V);
        keys.insert(EvdevKey::KEY_W);
        keys.insert(EvdevKey::KEY_X);
        keys.insert(EvdevKey::KEY_Y);
        keys.insert(EvdevKey::KEY_Z);

        keys.insert(EvdevKey::KEY_1);
        keys.insert(EvdevKey::KEY_2);
        keys.insert(EvdevKey::KEY_3);
        keys.insert(EvdevKey::KEY_4);
        keys.insert(EvdevKey::KEY_5);
        keys.insert(EvdevKey::KEY_6);
        keys.insert(EvdevKey::KEY_7);
        keys.insert(EvdevKey::KEY_8);
        keys.insert(EvdevKey::KEY_9);
        keys.insert(EvdevKey::KEY_0);

        // Add punctuation and special keys
        keys.insert(EvdevKey::KEY_SPACE);
        keys.insert(EvdevKey::KEY_ENTER);
        keys.insert(EvdevKey::KEY_TAB);
        keys.insert(EvdevKey::KEY_LEFTSHIFT);
        keys.insert(EvdevKey::KEY_MINUS);
        keys.insert(EvdevKey::KEY_EQUAL);
        keys.insert(EvdevKey::KEY_KPPLUS);
        keys.insert(EvdevKey::KEY_LEFTBRACE);
        keys.insert(EvdevKey::KEY_RIGHTBRACE);
        keys.insert(EvdevKey::KEY_BACKSLASH);
        keys.insert(EvdevKey::KEY_SEMICOLON);
        keys.insert(EvdevKey::KEY_APOSTROPHE);
        keys.insert(EvdevKey::KEY_GRAVE);
        keys.insert(EvdevKey::KEY_COMMA);
        keys.insert(EvdevKey::KEY_DOT);
        keys.insert(EvdevKey::KEY_SLASH);

        keys.insert(EvdevKey::KEY_BACKSPACE);
        keys.insert(EvdevKey::KEY_ESC);
        keys.insert(EvdevKey::KEY_LEFT);
        keys.insert(EvdevKey::KEY_UP);

        keys.insert(EvdevKey::KEY_LEFTCTRL);
        keys.insert(EvdevKey::KEY_LEFTALT);

        VirtualDevice::builder()
            .unwrap()
            .name("Virtual Keyboard")
            .with_keys(&keys)
            .unwrap()
            .build()
            .unwrap()
    }

    fn create_key_map(rm2_keyboard: bool) -> HashMap<char, (EvdevKey, bool)> {
        let mut key_map = HashMap::new();

        // Lowercase letters
        key_map.insert('a', (EvdevKey::KEY_A, false));
        key_map.insert('b', (EvdevKey::KEY_B, false));
        key_map.insert('c', (EvdevKey::KEY_C, false));
        key_map.insert('d', (EvdevKey::KEY_D, false));
        key_map.insert('e', (EvdevKey::KEY_E, false));
        key_map.insert('f', (EvdevKey::KEY_F, false));
        key_map.insert('g', (EvdevKey::KEY_G, false));
        key_map.insert('h', (EvdevKey::KEY_H, false));
        key_map.insert('i', (EvdevKey::KEY_I, false));
        key_map.insert('j', (EvdevKey::KEY_J, false));
        key_map.insert('k', (EvdevKey::KEY_K, false));
        key_map.insert('l', (EvdevKey::KEY_L, false));
        key_map.insert('m', (EvdevKey::KEY_M, false));
        key_map.insert('n', (EvdevKey::KEY_N, false));
        key_map.insert('o', (EvdevKey::KEY_O, false));
        key_map.insert('p', (EvdevKey::KEY_P, false));
        key_map.insert('q', (EvdevKey::KEY_Q, false));
        key_map.insert('r', (EvdevKey::KEY_R, false));
        key_map.insert('s', (EvdevKey::KEY_S, false));
        key_map.insert('t', (EvdevKey::KEY_T, false));
        key_map.insert('u', (EvdevKey::KEY_U, false));
        key_map.insert('v', (EvdevKey::KEY_V, false));
        key_map.insert('w', (EvdevKey::KEY_W, false));
        key_map.insert('x', (EvdevKey::KEY_X, false));
        key_map.insert('y', (EvdevKey::KEY_Y, false));
        key_map.insert('z', (EvdevKey::KEY_Z, false));

        // Uppercase letters
        key_map.insert('A', (EvdevKey::KEY_A, true));
        key_map.insert('B', (EvdevKey::KEY_B, true));
        key_map.insert('C', (EvdevKey::KEY_C, true));
        key_map.insert('D', (EvdevKey::KEY_D, true));
        key_map.insert('E', (EvdevKey::KEY_E, true));
        key_map.insert('F', (EvdevKey::KEY_F, true));
        key_map.insert('G', (EvdevKey::KEY_G, true));
        key_map.insert('H', (EvdevKey::KEY_H, true));
        key_map.insert('I', (EvdevKey::KEY_I, true));
        key_map.insert('J', (EvdevKey::KEY_J, true));
        key_map.insert('K', (EvdevKey::KEY_K, true));
        key_map.insert('L', (EvdevKey::KEY_L, true));
        key_map.insert('M', (EvdevKey::KEY_M, true));
        key_map.insert('N', (EvdevKey::KEY_N, true));
        key_map.insert('O', (EvdevKey::KEY_O, true));
        key_map.insert('P', (EvdevKey::KEY_P, true));
        key_map.insert('Q', (EvdevKey::KEY_Q, true));
        key_map.insert('R', (EvdevKey::KEY_R, true));
        key_map.insert('S', (EvdevKey::KEY_S, true));
        key_map.insert('T', (EvdevKey::KEY_T, true));
        key_map.insert('U', (EvdevKey::KEY_U, true));
        key_map.insert('V', (EvdevKey::KEY_V, true));
        key_map.insert('W', (EvdevKey::KEY_W, true));
        key_map.insert('X', (EvdevKey::KEY_X, true));
        key_map.insert('Y', (EvdevKey::KEY_Y, true));
        key_map.insert('Z', (EvdevKey::KEY_Z, true));

        // Numbers
        key_map.insert('0', (EvdevKey::KEY_0, false));
        key_map.insert('1', (EvdevKey::KEY_1, false));
        key_map.insert('2', (EvdevKey::KEY_2, false));
        key_map.insert('3', (EvdevKey::KEY_3, false));
        key_map.insert('4', (EvdevKey::KEY_4, false));
        key_map.insert('5', (EvdevKey::KEY_5, false));
        key_map.insert('6', (EvdevKey::KEY_6, false));
        key_map.insert('7', (EvdevKey::KEY_7, false));
        key_map.insert('8', (EvdevKey::KEY_8, false));
        key_map.insert('9', (EvdevKey::KEY_9, false));

        // Special characters
        key_map.insert('!', (EvdevKey::KEY_1, true));
        key_map.insert('@', (EvdevKey::KEY_2, true));
        key_map.insert('#', (EvdevKey::KEY_3, true));
        key_map.insert('$', (EvdevKey::KEY_4, true));
        key_map.insert('%', (EvdevKey::KEY_5, true));
        key_map.insert('^', (EvdevKey::KEY_6, true));
        key_map.insert('&', (EvdevKey::KEY_7, true));
        key_map.insert('*', (EvdevKey::KEY_8, true));
        key_map.insert('(', (EvdevKey::KEY_9, true));
        key_map.insert(')', (EvdevKey::KEY_0, true));
        key_map.insert('_', (EvdevKey::KEY_MINUS, true));
        key_map.insert(
            '+',
            if rm2_keyboard {
                (EvdevKey::KEY_KPPLUS, false)
            } else {
                (EvdevKey::KEY_EQUAL, true)
            },
        );
        key_map.insert('{', (EvdevKey::KEY_LEFTBRACE, true));
        key_map.insert('}', (EvdevKey::KEY_RIGHTBRACE, true));
        key_map.insert('|', (EvdevKey::KEY_BACKSLASH, true));
        key_map.insert(':', (EvdevKey::KEY_SEMICOLON, true));
        key_map.insert('"', (EvdevKey::KEY_APOSTROPHE, true));
        key_map.insert('<', (EvdevKey::KEY_COMMA, true));
        key_map.insert('>', (EvdevKey::KEY_DOT, true));
        key_map.insert('?', (EvdevKey::KEY_SLASH, true));
        key_map.insert('~', (EvdevKey::KEY_GRAVE, true));

        // Common punctuation
        key_map.insert('-', (EvdevKey::KEY_MINUS, false));
        key_map.insert('=', (EvdevKey::KEY_EQUAL, rm2_keyboard));
        key_map.insert('[', (EvdevKey::KEY_LEFTBRACE, false));
        key_map.insert(']', (EvdevKey::KEY_RIGHTBRACE, false));
        key_map.insert('\\', (EvdevKey::KEY_BACKSLASH, false));
        key_map.insert(';', (EvdevKey::KEY_SEMICOLON, false));
        key_map.insert('\'', (EvdevKey::KEY_APOSTROPHE, false));
        key_map.insert(',', (EvdevKey::KEY_COMMA, false));
        key_map.insert('.', (EvdevKey::KEY_DOT, false));
        key_map.insert('/', (EvdevKey::KEY_SLASH, false));
        key_map.insert('`', (EvdevKey::KEY_GRAVE, false));

        // Whitespace
        key_map.insert(' ', (EvdevKey::KEY_SPACE, false));
        key_map.insert('\t', (EvdevKey::KEY_TAB, false));
        key_map.insert('\n', (EvdevKey::KEY_ENTER, false));

        // Action keys, such as backspace, escape, ctrl, alt
        key_map.insert('\x08', (EvdevKey::KEY_BACKSPACE, false));
        key_map.insert('\x1b', (EvdevKey::KEY_ESC, false));

        key_map
    }

    pub fn string_to_keypresses(&mut self, input: &str) -> Result<()> {
        if let Some(device) = &mut self.device {
            // make sure we are synced before we start; this might be paranoia
            device.emit(&[InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0)])?;
            thread::sleep(time::Duration::from_millis(10));

            for c in input.chars() {
                if let Some(&(key, shift)) = self.key_map.get(&c) {
                    // Firmware 3.28 maps the equals sign through Alt+Shift.
                    if self.rm2_keyboard && c == '=' {
                        device.emit(&[InputEvent::new(
                            EvdevEventType::KEY.0,
                            EvdevKey::KEY_LEFTALT.code(),
                            1,
                        )])?;
                    }
                    if shift {
                        // Press Shift
                        device.emit(&[InputEvent::new(
                            EvdevEventType::KEY.0,
                            EvdevKey::KEY_LEFTSHIFT.code(),
                            1,
                        )])?;
                    }

                    thread::sleep(time::Duration::from_millis(10));
                    // Press key
                    device.emit(&[InputEvent::new(EvdevEventType::KEY.0, key.code(), 1)])?;

                    thread::sleep(time::Duration::from_millis(10));
                    // Release key
                    device.emit(&[InputEvent::new(EvdevEventType::KEY.0, key.code(), 0)])?;

                    thread::sleep(time::Duration::from_millis(10));
                    if shift {
                        // Release Shift
                        device.emit(&[InputEvent::new(
                            EvdevEventType::KEY.0,
                            EvdevKey::KEY_LEFTSHIFT.code(),
                            0,
                        )])?;
                    }

                    if self.rm2_keyboard && c == '=' {
                        device.emit(&[InputEvent::new(
                            EvdevEventType::KEY.0,
                            EvdevKey::KEY_LEFTALT.code(),
                            0,
                        )])?;
                    }
                    if self.rm2_keyboard && c == '^' {
                        // RM2 treats Shift+6 as a dead superscript key. Space
                        // commits a literal caret instead of consuming the next
                        // exponent character (e.g. turning ^-11 into superscript
                        // minus followed by baseline 11).
                        thread::sleep(time::Duration::from_millis(10));
                        device.emit(&[InputEvent::new(
                            EvdevEventType::KEY.0,
                            EvdevKey::KEY_SPACE.code(),
                            1,
                        )])?;
                        device.emit(&[InputEvent::new(
                            EvdevEventType::KEY.0,
                            EvdevKey::KEY_SPACE.code(),
                            0,
                        )])?;
                    }
                    // Sync event
                    device.emit(&[InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0)])?;
                    thread::sleep(time::Duration::from_millis(10));
                }
            }
        }
        Ok(())
    }

    fn key_cmd(&mut self, button: &str, shift: bool) -> Result<()> {
        self.key_down(EvdevKey::KEY_LEFTCTRL)?;
        if shift {
            self.key_down(EvdevKey::KEY_LEFTSHIFT)?;
        }
        self.string_to_keypresses(button)?;
        if shift {
            self.key_up(EvdevKey::KEY_LEFTSHIFT)?;
        }
        self.key_up(EvdevKey::KEY_LEFTCTRL)?;
        Ok(())
    }

    pub fn key_cmd_body(&mut self) -> Result<()> {
        self.key_cmd("3", false)?;
        Ok(())
    }

    pub fn owned_sysfs(&mut self) -> Result<Option<std::path::PathBuf>> {
        self.device
            .as_mut()
            .map(|device| device.get_syspath().map_err(Into::into))
            .transpose()
    }

    /// Only the guarded history backend may call this after proving ownership.
    /// A failed guard or write always releases keys, never compensates an edit.
    pub fn history_command(
        &mut self,
        command: crate::workflow::history::Command,
        guard: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        use crate::workflow::history::Command;
        anyhow::ensure!(self.device.is_some(), "Native history keyboard disabled");
        if let Command::DeleteSuffix {
            characters,
            paragraphs,
        } = command
        {
            anyhow::ensure!(
                (1..=crate::workflow::history::MAX_CHARACTERS).contains(&characters),
                "Invalid history selection size"
            );
            anyhow::ensure!(
                (1..=crate::workflow::history::MAX_PARAGRAPHS).contains(&paragraphs),
                "Invalid history paragraph count"
            );
        }
        let deadline = time::Instant::now() + time::Duration::from_secs(60);
        let result = (|| -> Result<()> {
            let mut emit = |key, down| -> Result<()> {
                anyhow::ensure!(
                    time::Instant::now() < deadline,
                    "Native history key deadline exceeded"
                );
                guard()?;
                if down {
                    self.key_down(key)?;
                } else {
                    self.key_up(key)?;
                }
                thread::sleep(time::Duration::from_millis(50));
                guard()
            };
            match command {
                Command::DeleteSuffix { paragraphs, .. } => {
                    emit(EvdevKey::KEY_LEFTCTRL, true)?;
                    emit(EvdevKey::KEY_LEFTSHIFT, true)?;
                    for _ in 0..paragraphs {
                        emit(EvdevKey::KEY_UP, true)?;
                        emit(EvdevKey::KEY_UP, false)?;
                    }
                    emit(EvdevKey::KEY_LEFTSHIFT, false)?;
                    emit(EvdevKey::KEY_LEFTCTRL, false)?;
                    emit(EvdevKey::KEY_BACKSPACE, true)?;
                    emit(EvdevKey::KEY_BACKSPACE, false)?;
                }
                Command::RestoreDeletion | Command::RepeatDeletion => {
                    let key = if command == Command::RestoreDeletion {
                        EvdevKey::KEY_Z
                    } else {
                        EvdevKey::KEY_Y
                    };
                    emit(EvdevKey::KEY_LEFTCTRL, true)?;
                    emit(key, true)?;
                    emit(key, false)?;
                    emit(EvdevKey::KEY_LEFTCTRL, false)?;
                }
            }
            Ok(())
        })();
        let mut cleanup = Ok(());
        for key in [
            EvdevKey::KEY_LEFT,
            EvdevKey::KEY_UP,
            EvdevKey::KEY_BACKSPACE,
            EvdevKey::KEY_Z,
            EvdevKey::KEY_Y,
            EvdevKey::KEY_LEFTSHIFT,
            EvdevKey::KEY_LEFTCTRL,
        ] {
            if let Err(error) = self.key_up(key) {
                cleanup = Err(error);
            }
        }
        result?;
        cleanup
    }

    pub fn key_down(&mut self, key: EvdevKey) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.emit(&[(InputEvent::new(EvdevEventType::KEY.0, key.code(), 1))])?;
            device.emit(&[InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0)])?;
            thread::sleep(time::Duration::from_millis(1));
        }
        Ok(())
    }

    pub fn key_up(&mut self, key: EvdevKey) -> Result<()> {
        if let Some(device) = &mut self.device {
            device.emit(&[(InputEvent::new(EvdevEventType::KEY.0, key.code(), 0))])?;
            device.emit(&[InputEvent::new(EvdevEventType::SYNCHRONIZATION.0, 0, 0)])?;
            thread::sleep(time::Duration::from_millis(1));
        }
        Ok(())
    }

    pub fn progress(&mut self, note: &str) -> Result<()> {
        if self.no_draw_progress {
            return Ok(());
        }
        self.string_to_keypresses(note)?;
        self.progress_count += note.len() as u32;
        Ok(())
    }

    pub fn progress_end(&mut self) -> Result<()> {
        if self.no_draw_progress {
            return Ok(());
        }
        // Send a backspace for each progress
        for _ in 0..self.progress_count {
            self.string_to_keypresses("\x08")?;
        }
        self.progress_count = 0;
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
impl Keyboard {
    pub fn new(_no_draw: bool, no_draw_progress: bool) -> Self {
        Self {
            progress_count: 0,
            no_draw_progress,
        }
    }

    pub fn string_to_keypresses(&mut self, _input: &str) -> Result<()> {
        Ok(())
    }

    pub fn key_cmd_body(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn progress(&mut self, note: &str) -> Result<()> {
        if self.no_draw_progress {
            return Ok(());
        }
        self.progress_count += note.len() as u32;
        Ok(())
    }

    pub fn progress_end(&mut self) -> Result<()> {
        if self.no_draw_progress {
            return Ok(());
        }
        self.progress_count = 0;
        Ok(())
    }
}
