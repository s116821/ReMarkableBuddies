//! Main-thread systemd notifications. These establish service liveness only.
use anyhow::{ensure, Context, Result};
use std::{
    ffi::OsString,
    os::{
        linux::net::SocketAddrExt,
        unix::{
            ffi::OsStrExt,
            net::{SocketAddr, UnixDatagram},
        },
    },
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(120);
const CADENCE: Duration = Duration::from_secs(30);

/// Owned by the ordinary runtime's main thread; never serviced by a timer thread.
pub struct ServiceHealth {
    socket: UnixDatagram,
    pid: u32,
    thread: ThreadId,
    last_sent: Option<Instant>,
    last_seen: Option<Instant>,
    failed: Arc<AtomicBool>,
}

impl ServiceHealth {
    /// Capture once before dotenv/threads. Absence retains unsupervised behavior;
    /// partial or unsupported notification settings refuse startup.
    pub fn from_environment() -> Result<Option<Self>> {
        let Some(address) = settings(
            std::env::var_os("NOTIFY_SOCKET"),
            std::env::var_os("WATCHDOG_USEC"),
            std::env::var_os("WATCHDOG_PID"),
            std::process::id(),
        )?
        else {
            return Ok(None);
        };
        let socket = UnixDatagram::unbound().context("Create service notification socket")?;
        socket.set_nonblocking(true)?;
        socket
            .connect_addr(&address)
            .context("Connect service notification socket")?;
        Ok(Some(Self::new(socket)))
    }

    fn new(socket: UnixDatagram) -> Self {
        Self {
            socket,
            pid: std::process::id(),
            thread: thread::current().id(),
            last_sent: None,
            last_seen: None,
            failed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Called only after all ordinary initialization has succeeded.
    pub fn ready(&mut self) -> Result<()> {
        self.notify(Instant::now(), true)
    }

    /// Called after a successful main-thread input poll or request-progress check.
    pub fn progress(&mut self) -> Result<()> {
        self.notify(Instant::now(), false)
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Arm before attempting lease publication, which can leave partial state.
    /// The successful session retains this guard for its entire lifetime.
    pub(super) fn exit_on_drop(&mut self) -> Result<ExitOnDrop> {
        self.progress()?;
        Ok(ExitOnDrop {
            failed: self.failed.clone(),
        })
    }

    fn notify(&mut self, now: Instant, ready: bool) -> Result<()> {
        let result = self.notify_inner(now, ready);
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }

    fn notify_inner(&mut self, now: Instant, ready: bool) -> Result<()> {
        ensure!(!self.failed(), "Service health failed; runtime must exit");
        ensure!(
            self.pid == std::process::id() && self.thread == thread::current().id(),
            "Service health owner changed"
        );
        ensure!(
            self.last_seen.is_none_or(|seen| now >= seen),
            "Service health clock regressed"
        );
        if ready {
            ensure!(
                self.last_sent.is_none(),
                "Service readiness already reported"
            );
        } else {
            let last = self.last_sent.context("Service readiness not reported")?;
            let elapsed = now
                .checked_duration_since(last)
                .context("Service health clock regressed")?;
            ensure!(
                elapsed < WINDOW,
                "Service health expired; runtime must exit"
            );
            if elapsed < CADENCE {
                self.last_seen = Some(now);
                return Ok(());
            }
        }
        let message: &[u8] = if ready {
            b"READY=1\nWATCHDOG=1"
        } else {
            b"WATCHDOG=1"
        };
        // A full or failed endpoint is fatal. Never wait/retry or refresh on failure.
        ensure!(
            self.socket
                .send(message)
                .context("Send service notification")?
                == message.len(),
            "Incomplete service notification"
        );
        self.last_sent = Some(now);
        self.last_seen = Some(now);
        Ok(())
    }
}

/// A consumed activation cannot leave the ordinary loop alive without its session.
/// No reset/disarm exists and no stock-service command runs in this destructor.
pub(super) struct ExitOnDrop {
    failed: Arc<AtomicBool>,
}
impl ExitOnDrop {
    pub(super) fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }
    pub(super) fn check(&self, health: &mut ServiceHealth) -> Result<()> {
        if !Arc::ptr_eq(&self.failed, &health.failed) {
            self.fail();
            anyhow::bail!("Activation health owner changed");
        }
        health.progress()
    }
}
impl Drop for ExitOnDrop {
    fn drop(&mut self) {
        self.fail();
    }
}

fn settings(
    socket: Option<OsString>,
    interval: Option<OsString>,
    pid: Option<OsString>,
    own_pid: u32,
) -> Result<Option<SocketAddr>> {
    if socket.is_none() && interval.is_none() && pid.is_none() {
        return Ok(None);
    }
    let socket = socket.context("Missing service notification socket")?;
    ensure!(
        interval.as_deref() == Some(std::ffi::OsStr::new("120000000")),
        "Unsupported service watchdog interval"
    );
    if let Some(pid) = pid {
        let pid = pid.to_str().context("Invalid service watchdog owner")?;
        ensure!(
            !pid.is_empty()
                && pid.bytes().all(|b| b.is_ascii_digit())
                && pid.parse::<u32>().ok() == Some(own_pid),
            "Service watchdog owner mismatch"
        );
    }
    let bytes = socket.as_bytes();
    ensure!(!bytes.contains(&0), "Invalid service notification socket");
    let address = if bytes.first() == Some(&b'@') {
        ensure!(
            bytes.len() > 1,
            "Empty abstract service notification socket"
        );
        SocketAddr::from_abstract_name(&bytes[1..])?
    } else {
        ensure!(
            Path::new(&socket).is_absolute(),
            "Service notification socket must be absolute"
        );
        SocketAddr::from_pathname(&socket)?
    };
    Ok(Some(address))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn pair() -> (ServiceHealth, UnixDatagram) {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        sender.set_nonblocking(true).unwrap();
        receiver.set_nonblocking(true).unwrap();
        (ServiceHealth::new(sender), receiver)
    }
    fn receive(socket: &UnixDatagram, expected: &[u8]) {
        let mut bytes = [0u8; 64];
        let size = socket.recv(&mut bytes).unwrap();
        assert_eq!(&bytes[..size], expected);
    }
    fn empty(socket: &UnixDatagram) {
        assert_eq!(
            socket.recv(&mut [0u8; 64]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn activation_lifetime_guard_cannot_leave_an_ordinary_loop_healthy() {
        let (mut health, receiver) = pair();
        assert!(health.exit_on_drop().is_err()); // READY is mandatory.
        empty(&receiver);
        for failure in 0..3 {
            let (mut health, receiver) = pair();
            health.ready().unwrap();
            receive(&receiver, b"READY=1\nWATCHDOG=1");
            let guard = health.exit_on_drop().unwrap();
            guard.check(&mut health).unwrap();
            assert!(!health.failed());
            if failure == 1 {
                guard.fail();
            }
            if failure == 2 {
                let (mut other, _) = pair();
                assert!(guard.check(&mut other).is_err());
                assert!(health.failed());
            }
            drop(guard); // Also models failure before publish returned a Lease.
            assert!(health.failed());
            assert!(health.progress().is_err());
            empty(&receiver);
        }
    }

    #[test]
    fn failed_health_stops_the_real_orchestrator_without_input_or_provider() {
        use crate::{
            conversation::Ledger,
            storage::{Store, StorePaths},
            workflow::{orchestrator::Orchestrator, Workflow},
        };
        struct NoProvider;
        impl crate::LLMEngine for NoProvider {
            fn add_text_content(&mut self, _: &str) {
                panic!("provider entered")
            }
            fn add_image_content(&mut self, _: &str) {
                panic!("provider entered")
            }
            fn clear_content(&mut self) {
                panic!("provider entered")
            }
            fn execute(&mut self) -> Result<String> {
                panic!("provider entered")
            }
        }
        let (mut health, receiver) = pair();
        health.ready().unwrap();
        receive(&receiver, b"READY=1\nWATCHDOG=1");
        drop(receiver);
        health.last_sent = Some(Instant::now() - CADENCE);
        assert!(health.progress().is_err());
        let mut workflow = Workflow::with_device(
            Box::new(crate::workflow::selected_backend::RefusingBackend::new()),
            false,
        );
        workflow.service_health = Some(health);
        assert!(workflow.check_request_progress().is_err());
        assert!(
            !workflow.input_failed(),
            "health failure is not user cancellation"
        );
        assert!(workflow.runtime_failed());
        let root = std::env::temp_dir().join(format!("health-loop-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(
            Store::open(StorePaths {
                data: root.join("data"),
                cache: root.join("cache"),
                credentials: root.join("credentials"),
            })
            .unwrap(),
        );
        let mut orchestrator = Orchestrator::new(workflow, NoProvider, Ledger::new(store));
        orchestrator.set_trigger_enabled(false);
        assert!(orchestrator.run_loop().is_err());
        drop(orchestrator);
        // The exact fresh UUID directory is owned by this test alone.
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_notifications_alone_renew_the_deadline() {
        let (mut health, receiver) = pair();
        let start = Instant::now();
        health.notify(start, true).unwrap();
        receive(&receiver, b"READY=1\nWATCHDOG=1");
        health
            .notify(start + Duration::from_secs(29), false)
            .unwrap();
        empty(&receiver);
        assert!(health.notify(start + WINDOW, false).is_err());
        assert!(health.failed());
        assert!(health
            .notify(start + Duration::from_secs(30), false)
            .is_err());
        empty(&receiver);

        let (mut health, receiver) = pair();
        health.notify(start, true).unwrap();
        receive(&receiver, b"READY=1\nWATCHDOG=1");
        for second in [30, 60, 90, 120, 150] {
            health
                .notify(start + Duration::from_secs(second), false)
                .unwrap();
            receive(&receiver, b"WATCHDOG=1");
        }
        health
            .notify(start + Duration::from_secs(269), false)
            .unwrap();
        receive(&receiver, b"WATCHDOG=1");
        assert!(!health.failed());
    }

    #[test]
    fn readiness_order_clock_and_owner_fail_permanently() {
        let now = Instant::now();
        let (mut health, receiver) = pair();
        assert!(health.notify(now, false).is_err());
        assert!(health.notify(now, true).is_err());
        empty(&receiver);
        let (mut health, receiver) = pair();
        health.notify(now, true).unwrap();
        receive(&receiver, b"READY=1\nWATCHDOG=1");
        health.notify(now + Duration::from_secs(20), false).unwrap();
        assert!(health.notify(now + Duration::from_secs(19), false).is_err());
        empty(&receiver);
        let (mut health, _) = pair();
        health.pid = health.pid.wrapping_add(1);
        assert!(health.ready().is_err());
        let (mut health, receiver) = pair();
        health.ready().unwrap();
        receive(&receiver, b"READY=1\nWATCHDOG=1");
        assert!(health.ready().is_err());
        empty(&receiver);
        let (mut health, receiver) = pair();
        let mut health = thread::spawn(move || {
            assert!(health.ready().is_err());
            health
        })
        .join()
        .unwrap();
        assert!(health.ready().is_err());
        empty(&receiver);
    }

    #[test]
    fn failed_or_full_endpoint_never_waits_or_recovers() {
        let (mut health, receiver) = pair();
        drop(receiver);
        assert!(health.ready().is_err());
        assert!(health.failed());
        let (mut health, receiver) = pair();
        let now = Instant::now();
        health.notify(now, true).unwrap();
        receive(&receiver, b"READY=1\nWATCHDOG=1");
        // Fill a real socket buffer without draining it, under a finite bound.
        let full = (0..100_000).any(|_| health.socket.send(b"fixture").is_err());
        assert!(full);
        let started = Instant::now();
        assert!(health.notify(now + CADENCE, false).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        while receiver.recv(&mut [0; 64]).is_ok() {}
        assert!(health.notify(now + CADENCE, false).is_err());
        empty(&receiver);
    }

    #[test]
    fn notification_descriptor_is_nonblocking_and_closes_on_exec() {
        let (health, _) = pair();
        let fd = health.socket.as_raw_fd();
        // SAFETY: fcntl only reads flags on the live owned descriptor.
        let (status, descriptor) = unsafe {
            (
                nix::libc::fcntl(fd, nix::libc::F_GETFL),
                nix::libc::fcntl(fd, nix::libc::F_GETFD),
            )
        };
        assert!(status >= 0 && status & nix::libc::O_NONBLOCK != 0);
        assert!(descriptor >= 0 && descriptor & nix::libc::FD_CLOEXEC != 0);
    }

    #[test]
    fn notification_configuration_is_complete_and_owned() {
        let config = |socket: Option<&str>, interval: Option<&str>, pid: Option<&str>| {
            settings(
                socket.map(Into::into),
                interval.map(Into::into),
                pid.map(Into::into),
                42,
            )
        };
        assert!(config(None, None, None).unwrap().is_none());
        assert!(config(Some("/run/systemd/notify"), Some("120000000"), None)
            .unwrap()
            .is_some());
        assert_eq!(
            config(Some("@fixture"), Some("120000000"), Some("42"))
                .unwrap()
                .unwrap()
                .as_abstract_name(),
            Some(b"fixture".as_slice())
        );
        for values in [
            (None, Some("120000000"), None),
            (Some("/run/notify"), None, None),
            (None, None, Some("42")),
            (Some("relative"), Some("120000000"), None),
            (Some(""), Some("120000000"), None),
            (Some("@"), Some("120000000"), None),
            (Some("@bad\0name"), Some("120000000"), None),
            (Some("/run/notify"), Some("0"), None),
            (Some("/run/notify"), Some("60000000"), None),
            (Some("/run/notify"), Some("120000000"), Some("43")),
            (Some("/run/notify"), Some("120000000"), Some("+42")),
        ] {
            assert!(config(values.0, values.1, values.2).is_err());
        }
    }
}
