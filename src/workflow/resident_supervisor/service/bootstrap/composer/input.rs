//! Fixed, selected actor input. This is neither caret positioning nor product Submit.
use super::*;
use crate::device::keyboard::Keyboard;
use evdev::KeyCode;
use std::{fs::Metadata, io::Read, os::unix::fs::FileTypeExt};

fn node(sysfs: &Path) -> Result<Option<(PathBuf, u64, u64)>> {
    let mut found = None;
    for entry in fs::read_dir("/dev/input")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name
            .strip_prefix("event")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        if fs::canonicalize(Path::new("/sys/class/input").join(name).join("device"))? != sysfs {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path())?;
        ensure!(
            meta.file_type().is_char_device() && found.is_none(),
            "ambiguous owned keyboard node"
        );
        found = Some((entry.path(), meta.ino(), meta.rdev()));
    }
    Ok(found)
}

pub(crate) struct Owner {
    keyboard: Keyboard,
    sysfs: PathBuf,
    node: (PathBuf, u64, u64),
    root: PathBuf,
    held: Option<KeyCode>,
    attempted_downs: usize,
    completed_pairs: usize,
    release_attempts: usize,
}
impl Owner {
    pub(super) fn open(root: &Path, deadline: Instant) -> Result<Self> {
        let mut keyboard = Keyboard::composer_trial()?;
        let sysfs = fs::canonicalize(
            keyboard
                .owned_sysfs()?
                .context("composer keyboard has no owned sysfs")?,
        )?;
        let end = deadline.min(Instant::now() + Duration::from_secs(2));
        let node = loop {
            check(end)?;
            if let Some(node) = node(&sysfs)? {
                break node;
            }
            pace(end);
        };
        write_new(
            root,
            "composer-keyboard.json",
            &serde_json::to_vec(&serde_json::json!({
            "sysfs":sysfs,"event":node.0,"inode":node.1,"device":node.2,"scope":"owned actor lifetime; no input emitted"}))?,
        )?;
        Ok(Self {
            keyboard,
            sysfs,
            node,
            root: root.to_owned(),
            held: None,
            attempted_downs: 0,
            completed_pairs: 0,
            release_attempts: 0,
        })
    }
    fn current(&self) -> Result<()> {
        ensure!(
            node(&self.sysfs)?.as_ref() == Some(&self.node),
            "owned keyboard inventory changed"
        );
        Ok(())
    }
    pub(crate) fn release(&mut self) -> Result<()> {
        if let Some(key) = self.held {
            self.release_attempts += 1;
            self.keyboard.key_up(key)?;
            self.held = None;
        }
        Ok(())
    }
    fn emit(
        &mut self,
        phase: Phase,
        expiry: u64,
        deadline: Instant,
        admit: &mut dyn FnMut(bool) -> Result<()>,
    ) -> Result<()> {
        self.attempted_downs = 0;
        self.completed_pairs = 0;
        self.release_attempts = 0;
        let end = deadline.min(Instant::now() + Duration::from_secs(4));
        let mut external = RequestInput::new(TriggerCorner::LowerLeft, Some(&self.sysfs))?;
        external.require_quiet()?;
        self.current()?;
        admit(true)?;
        check(end)?;
        remaining(expiry, 1_000_000_000)?;
        let mut down_ok = false;
        fixed_pulse(phase, &mut |step| match step {
            Step::Check => {
                self.current()?;
                admit(false)?;
                external.require_quiet()?;
                check(end)?;
                remaining(expiry, 1)
            }
            Step::Down(key) => {
                self.held = Some(key);
                self.attempted_downs += 1;
                let result = self.keyboard.key_down(key);
                down_ok = result.is_ok();
                result
            }
            Step::Up => {
                let result = self.release();
                if result.is_ok() && down_ok {
                    self.completed_pairs += 1;
                }
                result
            }
        })?;
        admit(false)?;
        external.require_quiet()?;
        check(end)?;
        remaining(expiry, 1)?;
        Ok(())
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if self.held.is_some() {
            let result = self.release();
            if let Ok(bytes) = serde_json::to_vec(
                &serde_json::json!({"result":format!("{result:?}"),"forced_kill_release_proven":false}),
            ) {
                let _ = write_new(&self.root, "composer-keyboard-drop-release.json", &bytes);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Seed,
    Typing,
}
#[derive(Clone, Copy)]
enum Step {
    Check,
    Down(KeyCode),
    Up,
}
fn fixed_pulse(phase: Phase, action: &mut dyn FnMut(Step) -> Result<()>) -> Result<()> {
    for &key in phase.keys() {
        action(Step::Check)?;
        // Down may have reached the kernel before a sync error. Up is therefore
        // unconditional, including after expiry; no later down follows failure.
        let down = action(Step::Down(key));
        let up = action(Step::Up);
        if down.is_err() || up.is_err() {
            anyhow::bail!("fixed key outcome down={down:?}; release={up:?}");
        }
        action(Step::Check)?;
    }
    Ok(())
}
impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Typing => "typing",
        }
    }
    fn keys(self) -> &'static [KeyCode] {
        use KeyCode as K;
        match self {
            Self::Seed => &[K::KEY_S, K::KEY_E, K::KEY_E, K::KEY_D, K::KEY_ENTER],
            Self::Typing => &[
                K::KEY_C,
                K::KEY_O,
                K::KEY_M,
                K::KEY_P,
                K::KEY_O,
                K::KEY_S,
                K::KEY_E,
                K::KEY_R,
                K::KEY_SPACE,
                K::KEY_T,
                K::KEY_R,
                K::KEY_I,
                K::KEY_A,
                K::KEY_L,
                K::KEY_ENTER,
            ],
        }
    }
}
fn remaining(expiry: u64, minimum: u64) -> Result<()> {
    remaining_at(expiry, monotonic_ns()?, minimum)
}
fn remaining_at(expiry: u64, now: u64, minimum: u64) -> Result<()> {
    let left = expiry.checked_sub(now).context("fixed input cue expired")?;
    ensure!(
        left >= minimum && left > 0 && left <= 20_000_000_000,
        "fixed input original budget invalid or insufficient"
    );
    Ok(())
}

fn identity(meta: &Metadata) -> (u64, u64, u32, u32, u64, u64, i64, i64, i64, i64) {
    (
        meta.dev(),
        meta.ino(),
        meta.uid(),
        meta.mode(),
        meta.nlink(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}
struct CuePin {
    directory: File,
    root: PathBuf,
    root_id: (u64, u64),
    file: File,
    metadata: Metadata,
    ready: &'static str,
    staged: &'static str,
    bytes: Vec<u8>,
}
impl CuePin {
    fn read(root: &Path, ready: &'static str, staged: &'static str) -> Result<Self> {
        let directory = private_root(root)?;
        let meta = directory.metadata()?;
        let mut file = open_at(&directory, ready, false, false)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == 0
                && metadata.mode() & 0o7777 == 0o600
                && metadata.nlink() == 2
                && metadata.len() <= 4096,
            "unsafe fixed input cue"
        );
        let mut bytes = Vec::new();
        Read::by_ref(&mut file).take(4097).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == metadata.len() && bytes.ends_with(b"\n"),
            "incomplete fixed input cue"
        );
        let value = Self {
            directory,
            root: root.to_owned(),
            root_id: (meta.dev(), meta.ino()),
            file,
            metadata,
            ready,
            staged,
            bytes,
        };
        value.current()?;
        Ok(value)
    }
    fn current(&self) -> Result<()> {
        let meta = private_root(&self.root)?.metadata()?;
        ensure!(
            (meta.dev(), meta.ino()) == self.root_id,
            "fixed input cue root replaced"
        );
        for meta in [
            self.file.metadata()?,
            open_at(&self.directory, self.ready, false, false)?.metadata()?,
            open_at(&self.directory, self.staged, false, false)?.metadata()?,
        ] {
            ensure!(
                identity(&meta) == identity(&self.metadata),
                "fixed input cue/staged pair changed"
            );
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypingCue {
    schema: u8,
    phase: String,
    binding: Message,
    input_mode: String,
    operation: String,
    native_armed_at_sample: String,
    native_expiry_ns: u64,
    text: String,
    diagnostic_key: String,
    live_lease_or_submit_claim: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn poll(
    context: &Context,
    message: &Message,
    owner: &mut Owner,
    seed_attempted: &mut bool,
    typing_attempted: &mut bool,
    deadline: Instant,
    admit: &mut dyn FnMut(bool) -> Result<()>,
) -> Result<()> {
    let phase = if !*seed_attempted {
        Phase::Seed
    } else if !*typing_attempted {
        Phase::Typing
    } else {
        return Ok(());
    };
    let (root, ready, staged) = match phase {
        Phase::Seed => (
            context.root.clone(),
            "composer.seed-ready",
            "composer.seed-staged",
        ),
        Phase::Typing => (
            context.stage.join("composer-store"),
            "typing-ready.json",
            "typing-cue.staged.json",
        ),
    };
    if !present(&root.join(ready))? {
        return Ok(());
    }
    let cue = CuePin::read(&root, ready, staged)?;
    let expiry = match phase {
        Phase::Seed => {
            let seed: SeedCue = serde_json::from_slice(&cue.bytes)?;
            ensure!(
                seed.schema == 1
                    && seed.phase == "composer-seed-v1"
                    && seed.message == *message
                    && uuid::Uuid::parse_str(&seed.cue_nonce).is_ok_and(|id| !id.is_nil()),
                "foreign fixed seed cue"
            );
            seed.expires_monotonic_ns
        }
        Phase::Typing => {
            let typing: TypingCue = serde_json::from_slice(&cue.bytes)?;
            ensure!(
                typing.schema == 1
                    && typing.phase == "composer-typing-v1"
                    && typing.binding == *message
                    && typing.input_mode == "actor_fixed"
                    && typing.text == "composer trial"
                    && typing.diagnostic_key == "Return once; ordinary newline is preserved"
                    && !typing.live_lease_or_submit_claim
                    && uuid::Uuid::parse_str(&typing.operation).is_ok_and(|id| !id.is_nil())
                    && typing
                        .native_armed_at_sample
                        .starts_with("RSDK_COMPOSER_ARMED_V1 ")
                    && typing
                        .native_armed_at_sample
                        .ends_with(&format!(" expiry_ns={}\n", typing.native_expiry_ns)),
                "foreign fixed typing cue"
            );
            typing.native_expiry_ns
        }
    };
    remaining(expiry, 1)?;
    let confirmation = context
        .root
        .join(format!("composer.{}-positioned", phase.name()));
    if !present(&confirmation)? {
        return Ok(());
    }
    let (confirmation_pin, bytes) = read_pin(&confirmation, deadline)?;
    ensure!(
        bytes == cue.bytes,
        "placement confirmation differs from complete current cue"
    );
    // Confirmation is an explicit operator assertion of visible caret placement
    // and released input. Arming/provisioning alone never implies either fact.
    admit(true)?;
    cue.current()?;
    confirmation_pin.current()?;
    remaining(expiry, 1_000_000_000)?;
    match phase {
        Phase::Seed => *seed_attempted = true,
        Phase::Typing => *typing_attempted = true,
    };
    write_new(
        &context.root,
        &format!("composer.{}-input-intent", phase.name()),
        &serde_json::to_vec(&serde_json::json!({
        "cue_sha256":sha(&cue.bytes),"expiry_monotonic_ns":expiry,"fixed_keys":phase.keys().len(),"retry":false}))?,
    )?;
    let result = owner.emit(phase, expiry, deadline, &mut |full| {
        cue.current()?;
        confirmation_pin.current()?;
        admit(full)
    });
    let release = owner.release();
    write_new(
        &context.root,
        &format!("composer.{}-input-result", phase.name()),
        &serde_json::to_vec(&serde_json::json!({
        "cue_sha256":sha(&cue.bytes),"result":format!("{result:?}"),"release":format!("{release:?}"),
        "attempted_downs":owner.attempted_downs,"completed_pairs":owner.completed_pairs,"release_attempts":owner.release_attempts,"key_may_remain_down":owner.held.is_some(),
        "failure_may_be_partial":result.is_err(),"native_text_or_physical_release_proof":false,"retry":false}))?,
    )?;
    result?;
    release?;
    ensure!(
        completed(
            phase.keys().len(),
            owner.attempted_downs,
            owner.completed_pairs,
            owner.release_attempts,
            owner.held.is_some()
        ),
        "incomplete fixed key sequence"
    );
    cue.current()?;
    confirmation_pin.current()?;
    admit(false)?;
    remaining(expiry, 1)?;
    check(deadline)?;
    if matches!(phase, Phase::Seed) {
        write_new(&context.root, "composer.seed-complete", &cue.bytes)?;
    } else {
        write_new(&context.root, "composer.typing-complete", &cue.bytes)?;
        // The observer must not treat Return's historical callback as release
        // completion. Publish only after successful final guards and full record.
        cue.current()?;
        confirmation_pin.current()?;
        admit(false)?;
        remaining(expiry, 1)?;
        check(deadline)?;
        write_new(&context.root, "composer.typing-complete.ready", b"")?;
    }
    Ok(())
}
fn completed(expected: usize, downs: usize, pairs: usize, releases: usize, held: bool) -> bool {
    expected > 0 && downs == expected && pairs == expected && releases == expected && !held
}

#[cfg(test)]
mod composer_controls {
    use super::*;
    #[test]
    fn return_down_alone_cannot_complete_fixed_typing() {
        assert!(completed(15, 15, 15, 15, false));
        assert!(!completed(15, 15, 14, 14, true));
        assert!(!completed(15, 15, 14, 15, false));
        assert!(!completed(15, 15, 15, 16, false));
        assert!(!completed(15, 14, 14, 14, false));
    }
    #[test]
    fn fixed_input_refuses_expiry_and_never_retries_partial_key() {
        assert!(remaining_at(100, 100, 1).is_err());
        assert!(remaining_at(100, 101, 1).is_err());
        assert!(remaining_at(100, 99, 2).is_err());
        for fail in ["first-check", "down", "up", "after-pair"] {
            let mut checks = 0;
            let mut events = Vec::new();
            let result = fixed_pulse(Phase::Typing, &mut |step| match step {
                Step::Check => {
                    checks += 1;
                    if (fail == "first-check" && checks == 1)
                        || (fail == "after-pair" && checks == 2)
                    {
                        anyhow::bail!("original deadline or input revoked");
                    }
                    Ok(())
                }
                Step::Down(key) => {
                    events.push((key.code(), true));
                    if fail == "down" {
                        anyhow::bail!("sync failed after possible press");
                    }
                    Ok(())
                }
                Step::Up => {
                    events.push((KeyCode::KEY_C.code(), false));
                    if fail == "up" {
                        anyhow::bail!("release failed");
                    }
                    Ok(())
                }
            });
            assert!(result.is_err());
            if fail == "first-check" {
                assert!(events.is_empty());
            } else {
                assert_eq!(
                    events,
                    vec![
                        (KeyCode::KEY_C.code(), true),
                        (KeyCode::KEY_C.code(), false)
                    ]
                );
            }
        }
    }
    #[test]
    fn fixed_input_has_only_selected_literal_keys_and_paired_release() {
        assert_eq!(
            Phase::Seed.keys(),
            &[
                KeyCode::KEY_S,
                KeyCode::KEY_E,
                KeyCode::KEY_E,
                KeyCode::KEY_D,
                KeyCode::KEY_ENTER
            ]
        );
        for phase in [Phase::Seed, Phase::Typing] {
            let mut held = None;
            let mut keys = Vec::new();
            let mut checks = 0;
            fixed_pulse(phase, &mut |step| {
                match step {
                    Step::Check => {
                        assert!(held.is_none());
                        checks += 1;
                    }
                    Step::Down(key) => {
                        assert!(held.replace(key).is_none());
                        keys.push(key);
                    }
                    Step::Up => {
                        assert!(held.take().is_some());
                    }
                }
                Ok(())
            })
            .unwrap();
            assert_eq!(keys, phase.keys());
            assert_eq!(checks, phase.keys().len() * 2);
            assert!(held.is_none());
        }
    }
}
