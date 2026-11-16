//! Text-free diagnostic witness from the exact native AwaitKey attempt. No live
//! lease follows from a log sample. The terminal key/draft checks remain required.
use anyhow::{ensure, Context, Result};
use remarkable_open_sdk::{
    native_adapter::{composer, Command, Frame, Message, RuntimeIdentity},
    OperationId,
};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

const PREFIX: &[u8] = b"RSDK_COMPOSER_ARMED_V1 ";
const MAX_LOG: u64 = 1024 * 1024;
const MAX_APPEND: u64 = 64 * 1024;
const MAX_LINE: usize = 4096;
fn identity(meta: &Metadata) -> (u64, u64, u32, u32, u64) {
    (
        meta.dev(),
        meta.ino(),
        meta.uid(),
        meta.mode(),
        meta.nlink(),
    )
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn monotonic_ns() -> Result<u64> {
    let mut time = std::mem::MaybeUninit::<nix::libc::timespec>::uninit();
    // SAFETY: valid output storage; CLOCK_MONOTONIC matches the explicit producer sample.
    ensure!(
        unsafe { nix::libc::clock_gettime(nix::libc::CLOCK_MONOTONIC, time.as_mut_ptr()) } == 0,
        "monotonic sample failed"
    );
    // SAFETY: successful clock_gettime initialized the timespec.
    let time = unsafe { time.assume_init() };
    ensure!(
        time.tv_sec >= 0 && (0..1_000_000_000).contains(&time.tv_nsec),
        "invalid monotonic sample"
    );
    u64::try_from(time.tv_sec)?
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(time.tv_nsec as u64))
        .context("monotonic overflow")
}
fn marker(line: &[u8], expected: Option<&[u8]>, seen: bool, now: u64) -> Result<bool> {
    if !line.windows(PREFIX.len()).any(|part| part == PREFIX) {
        return Ok(false);
    }
    let expected = expected.context("arming marker before prepared request")?;
    ensure!(
        !seen && line.starts_with(expected) && line.ends_with(b"\n"),
        "foreign, partial or duplicate native armed witness"
    );
    let digits = &line[expected.len()..line.len() - 1];
    ensure!(
        !digits.is_empty() && digits.len() <= 20 && digits.iter().all(u8::is_ascii_digit),
        "invalid armed expiry framing"
    );
    let expiry = std::str::from_utf8(digits)?.parse::<u64>()?;
    ensure!(
        expiry.to_string().as_bytes() == digits,
        "noncanonical armed expiry"
    );
    let remaining = expiry
        .checked_sub(now)
        .context("expired native armed witness")?;
    ensure!(
        (5_000_000_000..=20_000_000_000).contains(&remaining),
        "insufficient or invalid armed budget"
    );
    Ok(true)
}

pub struct Witness {
    path: PathBuf,
    directory: File,
    file: File,
    parent: Metadata,
    original: Metadata,
    start: u64,
    offset: u64,
    high_water: u64,
    partial: Vec<u8>,
    runtime: RuntimeIdentity,
    operation: OperationId,
    range: composer::RangeIdentity,
    keys: Vec<i32>,
    expected: Option<Vec<u8>>,
    seen: bool,
}
impl Witness {
    pub fn open(
        path: &Path,
        runtime: RuntimeIdentity,
        operation: OperationId,
        range: composer::RangeIdentity,
        keys: Vec<i32>,
    ) -> Result<Self> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(path)?;
        let parent = directory.metadata()?;
        ensure!(
            parent.is_dir()
                && parent.uid() == unsafe { nix::libc::geteuid() }
                && parent.mode() & 0o7777 == 0o700,
            "unsafe native log directory"
        );
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(format!(
                "/proc/self/fd/{}/stderr.log",
                directory.as_raw_fd()
            ))?;
        let original = file.metadata()?;
        ensure!(
            original.is_file()
                && original.uid() == parent.uid()
                && original.mode() & 0o7777 == 0o600
                && original.nlink() == 1
                && original.len() <= MAX_LOG,
            "unsafe native stderr"
        );
        let start = original.len();
        let value = Self {
            path: path.to_owned(),
            directory,
            file,
            parent,
            original,
            start,
            offset: start,
            high_water: start,
            partial: Vec::new(),
            runtime,
            operation,
            range,
            keys,
            expected: None,
            seen: false,
        };
        value.current()?;
        Ok(value)
    }
    fn current(&self) -> Result<u64> {
        ensure!(
            identity(&self.directory.metadata()?) == identity(&self.parent)
                && identity(&fs::symlink_metadata(&self.path)?) == identity(&self.parent),
            "native log directory replaced"
        );
        let descriptor = self.file.metadata()?;
        let named = fs::symlink_metadata(self.path.join("stderr.log"))?;
        ensure!(
            identity(&descriptor) == identity(&self.original)
                && identity(&named) == identity(&self.original)
                && descriptor.len() >= self.high_water
                && descriptor.len() <= MAX_LOG
                && descriptor.len().saturating_sub(self.start) <= MAX_APPEND,
            "native log replaced, truncated or over bound"
        );
        Ok(descriptor.len())
    }
    pub fn prepare(&mut self, frame: &Frame, digest: [u8; 32]) -> Result<()> {
        ensure!(
            self.expected.is_none()
                && frame.operation == self.operation
                && frame.scope.runtime == self.runtime
                && frame.fingerprint()? == digest,
            "wrong or repeated prepared AwaitKey frame"
        );
        ensure!(
            matches!(&frame.message,Message::Request(Command::Composer(composer::Command::AwaitKey{identity,keys}))
            if *identity==self.range && *keys==self.keys),
            "diagnosed command differs"
        );
        let bytes = frame.encode()?;
        // Existing v4 envelope: length(4), magic(4), version(2), runtime(140).
        ensure!(
            bytes.get(4..10) == Some(b"RSDK\0\x04".as_slice()),
            "unexpected prepared wire revision"
        );
        let runtime = bytes.get(10..150).context("short prepared runtime")?;
        self.expected=Some(format!("RSDK_COMPOSER_ARMED_V1 runtime={} client={} server={} sequence={} operation={} request_sha256={} visit={} range={} expiry_ns=",
            hex(runtime),hex(frame.scope.connection.as_bytes()),hex(frame.scope.server_nonce.context("missing prepared server nonce")?.as_bytes()),
            frame.sequence,hex(frame.operation.0.as_bytes()),hex(&digest),hex(self.range.visit.as_bytes()),hex(self.range.range.as_bytes())).into_bytes());
        self.current()?;
        Ok(())
    }
    /// Return the validated newly observed line once. Other complete stderr
    /// lines may be ignored; a foreign/duplicate/partial oversized marker fails.
    pub fn poll(&mut self) -> Result<Option<String>> {
        let size = self.current()?;
        self.high_water = size;
        let mut ready = None;
        while self.offset < size {
            let mut bytes = [0u8; 2048];
            let limit = usize::try_from((size - self.offset).min(bytes.len() as u64))?;
            let count = self.file.read_at(&mut bytes[..limit], self.offset)?;
            ensure!(count > 0, "native log shrank during read");
            self.offset += count as u64;
            for byte in &bytes[..count] {
                self.partial.push(*byte);
                ensure!(
                    self.partial.len() <= MAX_LINE,
                    "native stderr line over bound"
                );
                if *byte != b'\n' {
                    continue;
                }
                let line = std::mem::take(&mut self.partial);
                if marker(&line, self.expected.as_deref(), self.seen, monotonic_ns()?)? {
                    self.seen = true;
                    ready = Some(String::from_utf8(line)?);
                }
            }
        }
        self.current()?;
        Ok(ready)
    }
    pub fn seen(&self) -> bool {
        self.seen
    }
}

#[cfg(test)]
mod composer_armed_controls {
    use super::*;
    #[test]
    fn exact_complete_fresh_once_only() {
        let expected = b"RSDK_COMPOSER_ARMED_V1 actual-prepared-frame expiry_ns=";
        let valid = [expected.as_slice(), b"15000000000\n"].concat();
        assert!(marker(&valid, Some(expected), false, 1_000_000_000).unwrap());
        assert!(marker(&valid, Some(expected), true, 1_000_000_000).is_err());
        assert!(marker(&valid, None, false, 1_000_000_000).is_err());
        assert!(marker(
            &valid,
            Some(b"RSDK_COMPOSER_ARMED_V1 another-attempt "),
            false,
            1_000_000_000
        )
        .is_err());
        assert!(marker(
            &valid[..valid.len() - 1],
            Some(expected),
            false,
            1_000_000_000
        )
        .is_err());
        assert!(marker(&valid, Some(expected), false, 11_000_000_000).is_err());
        assert!(marker(&valid, Some(expected), false, 16_000_000_000).is_err());
        for expiry in [
            b"015000000000\n".as_slice(),
            b"15000000000\r\n",
            b"+15000000000\n",
            b"99999999999999999999\n",
            b"22000000000\n",
        ] {
            assert!(marker(
                &[expected.as_slice(), expiry].concat(),
                Some(expected),
                false,
                1_000_000_000
            )
            .is_err());
        }
        assert!(!marker(
            b"ordinary diagnostic\n",
            Some(expected),
            false,
            1_000_000_000
        )
        .unwrap());
        assert!(marker(
            &[b"prefix ".as_slice(), valid.as_slice()].concat(),
            Some(expected),
            false,
            1_000_000_000
        )
        .is_err());
    }
}
