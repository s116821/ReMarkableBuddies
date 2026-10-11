//! One development gesture; independently supervised, never UI authority.
#[cfg(all(target_os = "linux", feature = "development-input-diagnostics"))]
mod owned {
    use anyhow::{bail, ensure, Context, Result};
    use evdev::{Device, InputEvent};
    use nix::libc;
    use remarkable_reader_buddy::{
        device::{
            touch::{DiagnosticGestureEmitter, Touch, TriggerCorner},
            DeviceModel,
        },
        workflow::xochitl_integration::{NavigationDirection, XochitlIntegration},
    };
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::{
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd, RawFd},
            unix::{
                fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
                net::UnixStream,
                process::CommandExt,
            },
        },
        path::{Path, PathBuf},
        process::{Child, Command, ExitStatus},
        time::Duration,
    };
    type Event = [i32; 3];
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        nonce: String,
        boot: String,
        stock_pid: u32,
        stock_start: u64,
        document: String,
        page_ids: Vec<String>,
        current_index: usize,
        direction: String,
        root_dev: u64,
        root_ino: u64,
        source_observed_ms: i64,
        go_not_after_ms: i64,
        overall_ceiling_ms: i64,
        source_image_sha256: String,
        source_image_verified: bool,
        sidecar_sha256: [String; 3],
        page_map_sha256: String,
        executable_sha256: String,
    }
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Seed {
        ids: [i32; 32],
        x: [i32; 32],
        y: [i32; 32],
        selected: i32,
    }
    impl Seed {
        fn released(&self) -> bool {
            self.ids.iter().all(|n| *n == -1)
        }
        fn owned(&self, tag: i32, p: [i32; 2]) -> bool {
            self.selected == 0
                && self.ids[0] == tag
                && self.x[0] == p[0]
                && self.y[0] == p[1]
                && self.ids[1..].iter().all(|n| *n == -1)
        }
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Identity {
        dev: u64,
        ino: u64,
        rdev: u64,
    }
    fn identity(f: &File) -> Result<Identity> {
        let m = f.metadata()?;
        ensure!(m.file_type().is_char_device(), "Unknown input descriptor");
        Ok(Identity {
            dev: m.dev(),
            ino: m.ino(),
            rdev: m.rdev(),
        })
    }
    fn fd_identity(fd: RawFd) -> Result<Identity> {
        let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        ensure!(n >= 0, "Cannot duplicate input");
        identity(&unsafe { File::from_raw_fd(n) })
    }
    fn now() -> Result<i64> {
        let mut t: libc::timespec = unsafe { std::mem::zeroed() };
        ensure!(
            unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut t) } == 0
                && t.tv_sec >= 0
                && (0..1_000_000_000).contains(&t.tv_nsec),
            "Unknown BOOTTIME"
        );
        (t.tv_sec as i64)
            .checked_mul(1000)
            .and_then(|n| n.checked_add((t.tv_nsec as i64) / 1_000_000))
            .context("Clock overflow")
    }
    fn ior(n: u32, len: usize) -> libc::c_ulong {
        ((2u64 << 30) | ((len as u64) << 16) | (69 << 8) | n as u64) as _
    }
    fn snapshot(fd: RawFd) -> Result<Seed> {
        #[repr(C)]
        #[derive(Default)]
        struct Abs {
            value: i32,
            min: i32,
            max: i32,
            fuzz: i32,
            flat: i32,
            resolution: i32,
        }
        let mut slot = Abs::default();
        ensure!(
            unsafe { libc::ioctl(fd, ior(0x20 + 47, std::mem::size_of::<Abs>()), &mut slot) } == 0
                && slot.min == 0
                && slot.max == 31
                && (0..32).contains(&slot.value),
            "Unknown MT layout"
        );
        let axis = |code: i32| -> Result<[i32; 32]> {
            let mut a = [0i32; 33];
            a[0] = code;
            ensure!(
                unsafe { libc::ioctl(fd, ior(0x0a, std::mem::size_of_val(&a)), a.as_mut_ptr()) }
                    == 0,
                "Slot query failed"
            );
            Ok(a[1..].try_into().unwrap())
        };
        let ids = axis(57)?;
        let x = axis(53)?;
        let y = axis(54)?;
        ensure!(ids == axis(57)?, "Contact changed during query");
        let mut selected = Abs::default();
        ensure!(
            unsafe {
                libc::ioctl(
                    fd,
                    ior(0x20 + 47, std::mem::size_of::<Abs>()),
                    &mut selected,
                )
            } == 0
                && selected.value == slot.value
                && selected.min == 0
                && selected.max == 31,
            "Selected slot changed during query"
        );
        Ok(Seed {
            ids,
            x,
            y,
            selected: slot.value,
        })
    }
    fn bounded(p: &Path, cap: u64) -> Result<Vec<u8>> {
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(p)?;
        let m = f.metadata()?;
        ensure!(
            m.is_file() && m.nlink() == 1 && m.len() <= cap,
            "Unknown bounded file"
        );
        let mut b = Vec::new();
        Read::by_ref(&mut f).take(cap + 1).read_to_end(&mut b)?;
        ensure!(b.len() as u64 <= cap, "File grew");
        Ok(b)
    }
    fn sha(b: &[u8]) -> String {
        format!("{:x}", Sha256::digest(b))
    }
    fn hex(s: &str, n: usize) -> bool {
        s.len() == n
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }
    fn uuid(s: &str) -> bool {
        s.len() == 36
            && s.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
                }
            })
    }
    fn start(pid: u32) -> Result<u64> {
        let s = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        s.rsplit_once(')')
            .context("Malformed held process")?
            .1
            .split_whitespace()
            .nth(19)
            .context("No held start")?
            .parse()
            .context("Invalid held start")
    }
    fn validate(c: &Config, root: &Path) -> Result<()> {
        ensure!(
            *c == serde_json::from_slice::<Config>(&bounded(&root.join("config.json"), 16384)?)?,
            "Sealed selection changed"
        );
        ensure!(
            hex(&c.nonce, 32)
                && uuid(&c.boot)
                && uuid(&c.document)
                && c.stock_pid > 1
                && c.stock_start > 0,
            "Unknown selection"
        );
        ensure!(
            c.page_ids.len() == 8
                && c.current_index < 8
                && c.page_ids.iter().all(|s| uuid(s))
                && c.page_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == 8,
            "Exact eight-page order required"
        );
        ensure!(
            matches!(c.direction.as_str(), "next" | "previous"),
            "One fixed direction required"
        );
        ensure!(
            if c.direction == "next" {
                c.current_index + 1 < 8
            } else {
                c.current_index > 0
            },
            "No existing neighbor"
        );
        ensure!(
            root == Path::new(&format!("/run/rmb-gesture-{}", c.nonce)),
            "Unknown owned root"
        );
        let m = fs::symlink_metadata(root)?;
        ensure!(
            m.is_dir()
                && m.uid() == 0
                && m.mode() & 0o7777 == 0o700
                && m.dev() == c.root_dev
                && m.ino() == c.root_ino,
            "Root changed"
        );
        ensure!(
            bounded(&root.join("owner"), 32)? == c.nonce.as_bytes(),
            "Owner changed"
        );
        for n in ["owner", "config.json", "source.png"] {
            let m = fs::symlink_metadata(root.join(n))?;
            ensure!(
                m.is_file() && m.uid() == 0 && m.mode() & 0o7777 == 0o600 && m.nlink() == 1,
                "Private selection changed"
            );
        }
        ensure!(
            c.source_image_verified
                && hex(&c.source_image_sha256, 64)
                && hex(&c.executable_sha256, 64)
                && hex(&c.page_map_sha256, 64)
                && c.sidecar_sha256.iter().all(|s| hex(s, 64)),
            "Reviewed source/artifact required"
        );
        ensure!(
            c.source_observed_ms >= 0
                && c.go_not_after_ms >= c.source_observed_ms
                && c.overall_ceiling_ms
                    >= c.go_not_after_ms
                        .checked_add(5000)
                        .context("Deadline overflow")?,
            "Predeclared ceiling required"
        );
        ensure!(
            DeviceModel::detect() == DeviceModel::Remarkable2
                && remarkable_reader_buddy::device::native_page::verified_contract(
                    DeviceModel::Remarkable2,
                    &fs::read_to_string("/usr/lib/os-release")?
                ),
            "Unqualified device/firmware"
        );
        Ok(())
    }
    fn page_map(content: &serde_json::Value) -> Result<String> {
        ensure!(
            content["pageCount"].as_u64() == Some(8),
            "Page count changed"
        );
        let mut pages = Vec::new();
        for p in content["cPages"]["pages"]
            .as_array()
            .context("No page map")?
        {
            // Preserve presence as well as value, including tombstones and redirect semantics.
            pages.push(serde_json::json!({"id":p.get("id"),"idx":p.get("idx"),
            "deleted":p.get("deleted"),"redir_present":p.get("redir").is_some(),"redir":p.get("redir")}));
        }
        pages.sort_by_key(|p| p["id"].to_string());
        Ok(sha(&serde_json::to_vec(
            &serde_json::json!({"pageCount":8,"pages":pages}),
        )?))
    }
    fn owner_admission(
        source: usize,
        neighbor: usize,
        ci: usize,
        mi: usize,
        source_only: bool,
        release_only: bool,
    ) -> Result<bool> {
        let allowed = |i: usize| i == source || (!source_only && i == neighbor);
        ensure!(
            allowed(ci) && allowed(mi) && (release_only || ci == mi),
            "Unexpected/incoherent owner"
        );
        Ok(ci == mi)
    }
    fn context(c: &Config, root: &Path, source_only: bool, release_only: bool) -> Result<bool> {
        validate(c, root)?;
        ensure!(
            fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim() == c.boot
                && start(c.stock_pid)? == c.stock_start,
            "Stock session changed"
        );
        let paths = [
            PathBuf::from("/home/root/.config/remarkable/xochitl.conf"),
            PathBuf::from(format!(
                "/home/root/.local/share/remarkable/xochitl/{}.content",
                c.document
            )),
            PathBuf::from(format!(
                "/home/root/.local/share/remarkable/xochitl/{}.metadata",
                c.document
            )),
        ];
        let bytes = paths
            .iter()
            .map(|p| bounded(p, 65536))
            .collect::<Result<Vec<_>>>()?;
        let last = std::str::from_utf8(&bytes[0])?
            .lines()
            .filter_map(|s| s.strip_prefix("LastOpen="))
            .collect::<Vec<_>>();
        ensure!(
            last == [format!("@ByteArray({})", c.document)],
            "Actual document changed"
        );
        let content: serde_json::Value = serde_json::from_slice(&bytes[1])?;
        let meta: serde_json::Value = serde_json::from_slice(&bytes[2])?;
        ensure!(
            page_map(&content)? == c.page_map_sha256,
            "Immutable page map changed"
        );
        let mut order = Vec::new();
        for p in content["cPages"]["pages"].as_array().context("No order")? {
            if p["deleted"]["value"] == true {
                continue;
            }
            order.push((
                p["idx"]["value"]
                    .as_str()
                    .context("No ordering key")?
                    .to_owned(),
                p["id"].as_str().context("No page ID")?.to_owned(),
            ));
        }
        order.sort();
        ensure!(
            order.len() == 8
                && order.windows(2).all(|p| p[0].0 != p[1].0)
                && order.iter().map(|p| p.1.clone()).collect::<Vec<_>>() == c.page_ids,
            "Exact order changed"
        );
        let page = content["cPages"]["lastOpened"]["value"]
            .as_str()
            .context("No current page")?;
        let ci = c
            .page_ids
            .iter()
            .position(|p| p == page)
            .context("Unknown current owner")?;
        let mi = usize::try_from(
            meta["lastOpenedPage"]
                .as_u64()
                .context("No current metadata")?,
        )
        .context("Current index overflow")?;
        let neighbor = if c.direction == "next" {
            c.current_index + 1
        } else {
            c.current_index - 1
        };
        let coherent =
            owner_admission(c.current_index, neighbor, ci, mi, source_only, release_only)?;
        if source_only {
            ensure!(
                bytes
                    .iter()
                    .enumerate()
                    .all(|(i, b)| sha(b) == c.sidecar_sha256[i]),
                "Pre-DOWN bytes changed"
            );
        }
        for (p, b) in paths.iter().zip(&bytes) {
            ensure!(bounded(p, 65536)? == *b, "Context changed while sampled");
        }
        Ok(coherent)
    }
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Limits {
        go: i64,
        write: i64,
        recovery: i64,
        overall: i64,
    }
    impl Limits {
        fn fixed(go: i64, c: &Config) -> Result<Self> {
            ensure!(
                go >= c.source_observed_ms && go <= c.go_not_after_ms,
                "Late GO"
            );
            let write = go.checked_add(1500).context("Overflow")?;
            let recovery = go.checked_add(2500).context("Overflow")?;
            let overall = go.checked_add(5000).context("Overflow")?;
            ensure!(overall <= c.overall_ceiling_ms, "Ceiling cannot renew");
            Ok(Self {
                go,
                write,
                recovery,
                overall,
            })
        }
        fn normal(&self) -> Result<()> {
            let n = now()?;
            ensure!(n >= self.go && n < self.write, "Original write cutoff");
            Ok(())
        }
    }
    #[derive(Clone, Debug)]
    struct Policy {
        tag: i32,
        path: Vec<[i32; 2]>,
        seed: Seed,
        intent: Option<usize>,
        complete: Option<usize>,
        point: [i32; 2],
        selected: i32,
        active: bool,
        id_echo: bool,
        x_echo: bool,
        y_echo: bool,
        first_syn: bool,
        bad: bool,
        released: bool,
        count: usize,
        cursor: usize,
        up_echo: bool,
    }
    impl Policy {
        fn new(tag: i32, path: Vec<[i32; 2]>, seed: Seed) -> Result<Self> {
            ensure!(
                (1..=65535).contains(&tag)
                    && path.len() == 16
                    && seed.released()
                    && seed.x[0] != path[0][0],
                "Released seed/changed initial X required"
            );
            let point = [seed.x[0], seed.y[0]];
            let selected = seed.selected;
            Ok(Self {
                tag,
                path,
                seed,
                intent: None,
                complete: None,
                point,
                selected,
                active: false,
                id_echo: false,
                x_echo: false,
                y_echo: false,
                first_syn: false,
                bad: false,
                released: false,
                count: 0,
                cursor: 0,
                up_echo: false,
            })
        }
        fn evidence(&self) -> &str {
            if self.seed.y[0] == self.path[0][1] {
                "seed-carried"
            } else {
                "first-frame-echo"
            }
        }
        fn expected(&self, step: usize) -> Vec<Event> {
            let mut e = vec![[3, 47, 0], [3, 57, if step == 16 { -1 } else { self.tag }]];
            if step < 16 {
                e.extend([[3, 53, self.path[step][0]], [3, 54, self.path[step][1]]]);
                if step == 0 {
                    e.extend([[3, 58, 100], [3, 48, 17], [3, 49, 17], [3, 52, 4]]);
                }
            }
            e.push([0, 0, 0]);
            e
        }
        fn admit(&mut self, e: &[Event], s: &Seed) -> Result<usize> {
            ensure!(!self.bad && !self.released, "Gesture refused/released");
            let step = self.complete.map_or(0, |n| n + 1);
            ensure!(
                step <= 16 && e == self.expected(step) && self.intent != Some(step),
                "Unexpected/replayed batch"
            );
            if step == 0 {
                ensure!(*s == self.seed, "Fresh full seed changed");
            } else {
                ensure!(
                    s.owned(self.tag, self.path[step - 1]),
                    "Exact owned step changed"
                );
            }
            self.intent = Some(step);
            self.cursor = 0;
            Ok(step)
        }
        fn observe(&mut self, e: Event) -> Result<()> {
            self.count += 1;
            ensure!(self.count <= 4096, "Raw event cap");
            ensure!(!self.released, "Event after release");
            let step = self.intent.context("Event before intent")?;
            let expected = self.expected(step);
            let offset = expected[self.cursor..]
                .iter()
                .position(|v| *v == e)
                .context("Non-prefix/duplicate event")?;
            self.cursor += offset + 1;
            if e[0] == 0 {
                ensure!(e == [0, 0, 0], "Dropped/unknown SYN");
                if step == 16 {
                    ensure!(
                        !self.active && self.up_echo,
                        "Release still active/no UP echo"
                    );
                    self.released = true;
                    return Ok(());
                }
                if !self.first_syn {
                    self.first_syn = true;
                    ensure!(
                        self.id_echo
                            && self.x_echo
                            && (self.seed.y[0] == self.path[0][1] || self.y_echo),
                        "First frame lacks required coordinates"
                    );
                }
                ensure!(
                    self.active
                        && self.point == self.path[step]
                        && self.complete.map_or(step == 0, |p| step == p + 1),
                    "Nonmonotone/incomplete frame"
                );
                self.complete = Some(step);
                return Ok(());
            }
            ensure!(e[0] == 3, "Foreign event type");
            match e[1] {
                47 => {
                    ensure!(e[2] == 0, "Foreign slot");
                    self.selected = 0;
                }
                57 => {
                    ensure!(
                        self.selected == 0 && e[2] == if step == 16 { -1 } else { self.tag },
                        "Foreign tag"
                    );
                    self.active = step != 16;
                    self.id_echo = true;
                    if step == 16 {
                        self.up_echo = true;
                    }
                }
                53 | 54 => {
                    ensure!(
                        self.selected == 0
                            && step < 16
                            && e[2] == self.path[step][(e[1] - 53) as usize],
                        "Unexpected coordinate"
                    );
                    self.point[(e[1] - 53) as usize] = e[2];
                    if !self.first_syn {
                        if e[1] == 53 {
                            self.x_echo = true
                        } else {
                            self.y_echo = true
                        }
                    }
                }
                58 | 48 | 49 | 52 => ensure!(
                    step == 0
                        && e[2]
                            == match e[1] {
                                58 => 100,
                                48 | 49 => 17,
                                _ => 4,
                            },
                    "Unexpected attribute"
                ),
                _ => bail!("Unknown ABS"),
            }
            Ok(())
        }
        fn recovery_owned(&self, s: &Seed) -> bool {
            !self.bad
                && self.active
                && self.id_echo
                && self.intent.is_some()
                && s.owned(self.tag, self.point)
        }
    }
    fn path(c: &Config) -> Vec<[i32; 2]> {
        let touch = Touch::new(true, TriggerCorner::LowerLeft);
        let (a, b) = if c.direction == "next" {
            (700i32, 100i32)
        } else {
            (100, 700)
        };
        (0..=15)
            .map(|i| {
                let n = (((b - a).abs() as f32) * (i as f32 / 15.0)) as i32;
                let (x, y) = touch.diagnostic_native_point((a + (b - a).signum() * n, 512));
                [x, y]
            })
            .collect()
    }
    fn exclusive(p: &Path) -> Result<File> {
        Ok(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(p)?)
    }
    fn durable(root: &Path, name: &str, value: &impl Serialize) -> Result<()> {
        let mut f = exclusive(&root.join(name))?;
        serde_json::to_writer(&mut f, value)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        File::open(root)?.sync_all()?;
        Ok(())
    }

    fn contact_unresolved(run_ok: bool, death_known: bool, positive_cleanup: bool) -> bool {
        !death_known || (!run_ok && !positive_cleanup)
    }
    fn recovery_admission(
        policy: &Policy,
        state: &Seed,
        death: bool,
        at: i64,
        limits: &Limits,
    ) -> Result<()> {
        ensure!(
            death && at >= limits.go && at < limits.recovery,
            "No confirmed death within original recovery interval"
        );
        ensure!(
            policy.recovery_owned(state),
            "No exact retained-prefix ownership"
        );
        Ok(())
    }
    fn sticky_input<T>(policy: &mut Policy, result: Result<T>) -> Result<T> {
        if result.is_err() {
            policy.bad = true;
        }
        result
    }
    struct Inputs {
        files: Vec<(PathBuf, File, Identity)>,
        touch: usize,
        history: Option<File>,
    }
    fn input_names() -> Result<Vec<PathBuf>> {
        let mut names = fs::read_dir("/dev/input")?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        names.retain(|p| {
            p.file_name().and_then(|s| s.to_str()).is_some_and(|s| {
                s.strip_prefix("event")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
        });
        names.sort();
        ensure!(
            !names.is_empty() && names.len() <= 32,
            "Input inventory cap"
        );
        Ok(names)
    }
    impl Inputs {
        fn open(root: Option<&Path>) -> Result<Self> {
            let mut files = Vec::new();
            let mut touch = None;
            for p in input_names()? {
                let f = OpenOptions::new()
                    .read(true)
                    .write(p == Path::new("/dev/input/event2"))
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&p)?;
                let id = identity(&f)?;
                if p == Path::new("/dev/input/event2") {
                    touch = Some(files.len());
                }
                files.push((p, f, id));
            }
            ensure!(
                files
                    .iter()
                    .any(|(p, _, _)| p == Path::new("/dev/input/event1")),
                "Pen absent"
            );
            Ok(Self {
                files,
                touch: touch.context("Touch absent")?,
                history: root
                    .map(|r| exclusive(&r.join("raw-input.bin")))
                    .transpose()?,
            })
        }
        fn fd(&self) -> RawFd {
            self.files[self.touch].1.as_raw_fd()
        }
        fn validate(&self) -> Result<()> {
            ensure!(
                input_names()?
                    == self
                        .files
                        .iter()
                        .map(|(p, _, _)| p.clone())
                        .collect::<Vec<_>>(),
                "Input inventory changed"
            );
            for (p, f, id) in &self.files {
                let m = fs::symlink_metadata(p)?;
                ensure!(
                    identity(f)? == *id
                        && m.file_type().is_char_device()
                        && m.dev() == id.dev
                        && m.ino() == id.ino
                        && m.rdev() == id.rdev,
                    "Held input replaced"
                );
                let mut keys = [0u8; 96];
                let n =
                    unsafe { libc::ioctl(f.as_raw_fd(), ior(0x18, keys.len()), keys.as_mut_ptr()) };
                ensure!(
                    n == keys.len() as i32 && keys.iter().all(|b| *b == 0),
                    "Unknown/active key or pen"
                );
            }
            Ok(())
        }
        fn external_quiet(&self) -> Result<()> {
            for (i, (_, f, _)) in self.files.iter().enumerate() {
                if i == self.touch {
                    continue;
                }
                let mut e: libc::input_event = unsafe { std::mem::zeroed() };
                let n = unsafe {
                    libc::read(
                        f.as_raw_fd(),
                        (&mut e as *mut libc::input_event).cast(),
                        std::mem::size_of_val(&e),
                    )
                };
                ensure!(
                    n < 0
                        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock,
                    "Postpermission external input"
                );
            }
            Ok(())
        }
        fn identities(&self) -> Vec<Identity> {
            self.files.iter().map(|(_, _, id)| *id).collect()
        }
        fn drain(&mut self, mut policy: Option<&mut Policy>) -> Result<()> {
            let result = (|| -> Result<()> {
                for (i, (_, f, _)) in self.files.iter().enumerate() {
                    let mut exhausted = true;
                    for _ in 0..64 {
                        let mut events: [libc::input_event; 32] = unsafe { std::mem::zeroed() };
                        let n = unsafe {
                            libc::read(
                                f.as_raw_fd(),
                                events.as_mut_ptr().cast(),
                                std::mem::size_of_val(&events),
                            )
                        };
                        if n < 0 {
                            ensure!(
                                std::io::Error::last_os_error().kind()
                                    == std::io::ErrorKind::WouldBlock,
                                "Input read failed"
                            );
                            exhausted = false;
                            break;
                        }
                        ensure!(
                            n > 0 && n as usize % std::mem::size_of::<libc::input_event>() == 0,
                            "Malformed/partial input"
                        );
                        self.history
                            .as_mut()
                            .context("No raw retention")?
                            .write_all(unsafe {
                                std::slice::from_raw_parts(events.as_ptr().cast::<u8>(), n as usize)
                            })?;
                        self.history
                            .as_mut()
                            .context("No raw retention")?
                            .sync_data()?;
                        ensure!(i == self.touch, "External input");
                        let p = policy.as_deref_mut().context("Input before GO")?;
                        for e in events
                            .iter()
                            .take(n as usize / std::mem::size_of::<libc::input_event>())
                        {
                            p.observe([e.type_ as i32, e.code as i32, e.value])?;
                        }
                    }
                    ensure!(!exhausted, "Input drain cap");
                }
                Ok(())
            })();
            // Every foreign/unknown read is sticky, including non-touch descriptors.
            if result.is_err() {
                if let Some(p) = policy {
                    p.bad = true;
                }
            }
            result
        }
    }
    #[derive(Serialize, Deserialize)]
    struct Request {
        events: Vec<Event>,
        identity: Identity,
    }
    #[derive(Serialize, Deserialize)]
    struct Permission {
        write: bool,
        before: Seed,
        inventory: Vec<Identity>,
    }
    #[derive(Serialize, Deserialize)]
    struct Written {
        bytes: isize,
        coherent: bool,
        input_valid: bool,
    }
    fn send(socket: &mut UnixStream, value: &impl Serialize) -> Result<()> {
        let b = serde_json::to_vec(value)?;
        ensure!(b.len() < 8192, "Protocol cap");
        socket.write_all(&b)?;
        socket.write_all(b"\n")?;
        Ok(())
    }
    fn receive<T: for<'de> Deserialize<'de>>(socket: &mut UnixStream, cutoff: i64) -> Result<T> {
        let mut b = Vec::new();
        loop {
            let left = cutoff - now()?;
            ensure!(left > 0, "Original protocol cutoff");
            socket.set_read_timeout(Some(Duration::from_millis(left as u64)))?;
            let mut byte = [0];
            socket.read_exact(&mut byte)?;
            if byte[0] == b'\n' {
                break;
            }
            ensure!(b.len() < 8192, "Protocol cap");
            b.push(byte[0]);
        }
        Ok(serde_json::from_slice(&b)?)
    }
    fn accept_written(
        policy: &mut Policy,
        transition: &mut bool,
        written: &Written,
        expected: usize,
    ) -> Result<()> {
        if !written.coherent {
            *transition = true;
        }
        if !written.input_valid {
            policy.bad = true;
        }
        ensure!(
            written.bytes == expected as isize,
            "Partial batch; never replay"
        );
        Ok(())
    }
    struct Emitter {
        socket: UnixStream,
        c: Config,
        root: PathBuf,
        limits: Limits,
        tag: i32,
        inputs: Inputs,
    }
    impl DiagnosticGestureEmitter for Emitter {
        fn emit(&mut self, device: &mut Device, events: &[InputEvent]) -> Result<()> {
            self.limits.normal()?;
            let e = events
                .iter()
                .map(|e| [e.event_type().0 as i32, e.code() as i32, e.value()])
                .collect::<Vec<_>>();
            let fd = device.as_raw_fd();
            let id = fd_identity(fd)?;
            send(
                &mut self.socket,
                &Request {
                    events: e.clone(),
                    identity: id,
                },
            )?;
            let permission: Permission = receive(&mut self.socket, self.limits.write)?;
            if !permission.write {
                ensure!(permission.before.released(), "Unsafe skipped UP");
                return Ok(());
            }
            self.limits.normal()?;
            let coherent = context(&self.c, &self.root, e.len() == 9, e[1][2] == -1)?;
            let input_check = (|| -> Result<()> {
                self.inputs.validate()?;
                self.inputs.external_quiet()?;
                ensure!(
                    self.inputs.identities() == permission.inventory
                        && id == self.inputs.files[self.inputs.touch].2,
                    "Worker input binding drift"
                );
                ensure!(
                    snapshot(fd)? == permission.before && fd_identity(fd)? == id,
                    "Postpermission contact/device drift"
                );
                Ok(())
            })();
            if let Err(error) = input_check {
                send(
                    &mut self.socket,
                    &Written {
                        bytes: -1,
                        coherent,
                        input_valid: false,
                    },
                )?;
                return Err(error);
            }
            ensure!(e[1][2] == -1 || e[1][2] == self.tag, "Scoped tag changed");
            self.limits.normal()?;
            // One nonblocking syscall. A short/interrupted prefix is never retried.
            let n =
                unsafe { libc::write(fd, events.as_ptr().cast(), std::mem::size_of_val(events)) };
            send(
                &mut self.socket,
                &Written {
                    bytes: n,
                    coherent,
                    input_valid: true,
                },
            )?;
            ensure!(
                n == std::mem::size_of_val(events) as isize,
                "Partial/failed write"
            );
            Ok(())
        }
    }
    fn worker(root: &Path, tag: i32, limits: Limits, fd: RawFd) -> Result<()> {
        let c: Config = serde_json::from_slice(&bounded(&root.join("config.json"), 16384)?)?;
        validate(&c, root)?;
        ensure!(
            limits == Limits::fixed(limits.go, &c)?,
            "Worker ceilings changed"
        );
        limits.normal()?;
        let socket = unsafe { UnixStream::from_raw_fd(fd) };
        socket.set_write_timeout(Some(Duration::from_millis(5)))?;
        let inputs = Inputs::open(None)?;
        inputs.validate()?;
        inputs.external_quiet()?;
        let emitter = Emitter {
            socket,
            c: c.clone(),
            root: root.to_owned(),
            limits,
            tag,
            inputs,
        };
        let mut touch = Touch::diagnostic_scoped_gesture(tag, Box::new(emitter))?;
        XochitlIntegration::swipe(
            &mut touch,
            if c.direction == "next" {
                NavigationDirection::Next
            } else {
                NavigationDirection::Previous
            },
        )
    }
    fn stopped(child: &Child) -> Result<bool> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        ensure!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    child.id(),
                    &mut info,
                    libc::WEXITED | libc::WSTOPPED | libc::WNOHANG | libc::WNOWAIT,
                )
            } == 0,
            "Direct child state unknown"
        );
        Ok(unsafe { info.si_pid() } != 0 && info.si_code == libc::CLD_STOPPED)
    }
    fn fence(child: &mut Child, cutoff: i64, normal: bool) -> Result<ExitStatus> {
        if normal {
            let end = (now()? + 50).min(cutoff);
            while now()? < end {
                if let Some(status) = child.try_wait()? {
                    return Ok(status);
                }
                if stopped(child)? {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        child.kill()?; // Exact unreaped direct child handle: never an arbitrary PID.
        while now()? < cutoff {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        bail!("Writer death unconfirmed before original recovery cutoff")
    }
    fn raw_event(e: Event) -> libc::input_event {
        let mut v: libc::input_event = unsafe { std::mem::zeroed() };
        v.type_ = e[0] as u16;
        v.code = e[1] as u16;
        v.value = e[2];
        v
    }
    fn supervisor(root: &Path) -> Result<()> {
        let c: Config = serde_json::from_slice(&bounded(&root.join("config.json"), 16384)?)?;
        validate(&c, root)?;
        context(&c, root, true, false)?;
        let bytes = bounded(&root.join("source.png"), 16 * 1024 * 1024)?;
        ensure!(
            sha(&bytes) == c.source_image_sha256,
            "Reviewed source PNG changed"
        );
        let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?;
        ensure!(
            image.width() == 768 && image.height() == 1024,
            "Unknown source PNG dimensions"
        );
        let executable = std::env::current_exe()?;
        ensure!(
            sha(&bounded(&executable, 32 * 1024 * 1024)?) == c.executable_sha256,
            "Own artifact changed"
        );
        durable(root, "gesture.claim", &c.nonce)?;
        let mut inputs = Inputs::open(Some(root))?;
        inputs.validate()?;
        inputs.drain(None)?;
        let seed = snapshot(inputs.fd())?;
        inputs.validate()?;
        context(&c, root, true, false)?;
        ensure!(snapshot(inputs.fd())? == seed, "Setup seed changed");
        let mut random = [0u8; 4];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let mut ids = [0i32; 6];
        ensure!(
            unsafe {
                libc::ioctl(
                    inputs.fd(),
                    ior(0x20 + 57, std::mem::size_of_val(&ids)),
                    ids.as_mut_ptr(),
                )
            } == 0,
            "Tracking range unavailable"
        );
        let low = ids[1].max(1);
        let high = ids[2];
        ensure!(low <= high && high <= 65535, "Tracking range refused");
        let tag = low + (u32::from_ne_bytes(random) % (high - low + 1) as u32) as i32;
        let mut policy = Policy::new(tag, path(&c), seed)?;
        let limits = Limits::fixed(now()?, &c)?;
        durable(root, "gesture.go", &limits)?;
        limits.normal()?;
        inputs.drain(None)?;
        inputs.validate()?;
        context(&c, root, true, false)?;
        ensure!(snapshot(inputs.fd())? == policy.seed, "GO seed changed");
        let (mut socket, writer_socket) = UnixStream::pair()?;
        socket.set_write_timeout(Some(Duration::from_millis(5)))?;
        let fd = writer_socket.as_raw_fd();
        let mut command = Command::new(executable);
        command
            .arg("--worker")
            .arg(root)
            .arg(tag.to_string())
            .arg(serde_json::to_string(&limits)?)
            .arg(fd.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut intents = exclusive(&root.join("intents.jsonl"))?;
        let mut child = command.spawn()?;
        drop(writer_socket);
        let setup = (|| -> Result<()> {
            durable(
                root,
                "gesture.writer",
                &serde_json::json!({"pid":child.id(),"start":start(child.id())?,
        "tag":tag,"input":inputs.files[inputs.touch].2,"limits":limits}),
            )?;
            Ok(())
        })();
        let mut transition_unresolved = false;
        let run = (|| -> Result<()> {
            setup?;
            for expected_step in 0..=16 {
                limits.normal()?;
                ensure!(!stopped(&child)?, "Stopped writer");
                let request: Request = receive(&mut socket, limits.write)?;
                ensure!(
                    request.identity == inputs.files[inputs.touch].2,
                    "Writer descriptor differs"
                );
                inputs.drain(Some(&mut policy))?;
                sticky_input(&mut policy, inputs.validate())?;
                if !context(&c, root, expected_step == 0, expected_step == 16)? {
                    transition_unresolved = true;
                }
                let before = sticky_input(&mut policy, snapshot(inputs.fd()))?;
                let admission = policy.admit(&request.events, &before);
                let step = sticky_input(&mut policy, admission)?;
                ensure!(step == expected_step, "Unexpected step");
                serde_json::to_writer(
                    &mut intents,
                    &serde_json::json!({"step":step,"before":before,"events":request.events}),
                )?;
                intents.write_all(b"\n")?;
                intents.sync_all()?;
                // Every authority is checked after potentially blocking durable intent.
                limits.normal()?;
                inputs.drain(Some(&mut policy))?;
                sticky_input(&mut policy, inputs.validate())?;
                if !context(&c, root, step == 0, step == 16)? {
                    transition_unresolved = true;
                }
                let observed = sticky_input(&mut policy, snapshot(inputs.fd()))?;
                sticky_input(
                    &mut policy,
                    (|| -> Result<()> {
                        ensure!(observed == before, "Postintent seed/contact changed");
                        Ok(())
                    })(),
                )?;
                limits.normal()?;
                send(
                    &mut socket,
                    &Permission {
                        write: true,
                        before,
                        inventory: inputs.identities(),
                    },
                )?;
                let written: Written = receive(&mut socket, limits.write)?;
                accept_written(
                    &mut policy,
                    &mut transition_unresolved,
                    &written,
                    request.events.len() * std::mem::size_of::<libc::input_event>(),
                )?;
                let end = (now()? + 50).min(limits.write);
                loop {
                    limits.normal()?;
                    inputs.drain(Some(&mut policy))?;
                    sticky_input(&mut policy, inputs.validate())?;
                    if if step == 16 {
                        policy.released
                    } else {
                        policy.complete == Some(step)
                    } {
                        break;
                    }
                    ensure!(
                        now()? < end && !stopped(&child)?,
                        "Incomplete echo/stopped writer"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                let current = sticky_input(&mut policy, snapshot(inputs.fd()))?;
                let matches = if step == 16 {
                    current.released()
                } else {
                    current.owned(tag, policy.path[step])
                };
                sticky_input(
                    &mut policy,
                    (|| -> Result<()> {
                        ensure!(matches, "Postbatch contact mismatch");
                        Ok(())
                    })(),
                )?;
            }
            Ok(())
        })();
        let death = fence(&mut child, limits.recovery, run.is_ok());
        let mut recovered = false;

        if death.is_ok() && run.is_err() && now()? < limits.recovery {
            let recovery = (|| -> Result<bool> {
                inputs.drain(Some(&mut policy))?;
                sticky_input(&mut policy, inputs.validate())?;
                if !context(&c, root, false, true)? {
                    transition_unresolved = true;
                }
                let current = sticky_input(&mut policy, snapshot(inputs.fd()))?;
                if current.released() {
                    return Ok(true);
                }
                recovery_admission(&policy, &current, death.is_ok(), now()?, &limits)?;
                policy.intent = Some(16);
                policy.cursor = 0;
                durable(
                    root,
                    "gesture.up-intent",
                    &serde_json::json!({"point":policy.point,"tag":tag}),
                )?;
                ensure!(now()? < limits.recovery, "Original recovery cutoff");
                inputs.drain(Some(&mut policy))?;
                sticky_input(&mut policy, inputs.validate())?;
                if !context(&c, root, false, true)? {
                    transition_unresolved = true;
                }
                let observed = sticky_input(&mut policy, snapshot(inputs.fd()))?;
                sticky_input(
                    &mut policy,
                    (|| -> Result<()> {
                        ensure!(observed == current, "Recovery contact changed");
                        Ok(())
                    })(),
                )?;
                recovery_admission(&policy, &current, death.is_ok(), now()?, &limits)?;
                ensure!(now()? < limits.recovery, "Prewrite recovery cutoff");
                let e = [
                    raw_event([3, 47, 0]),
                    raw_event([3, 57, -1]),
                    raw_event([0, 0, 0]),
                ];
                let n = unsafe {
                    libc::write(inputs.fd(), e.as_ptr().cast(), std::mem::size_of_val(&e))
                };
                ensure!(
                    n == std::mem::size_of_val(&e) as isize,
                    "Partial UP; never replay"
                );
                while now()? < limits.recovery {
                    inputs.drain(Some(&mut policy))?;
                    sticky_input(&mut policy, inputs.validate())?;
                    if policy.released
                        && sticky_input(&mut policy, snapshot(inputs.fd()))?.released()
                    {
                        return Ok(true);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                bail!("No positive release within original recovery cutoff")
            })();
            recovered = recovery.as_ref().is_ok_and(|v| *v);
        }
        let unresolved = contact_unresolved(run.is_ok(), death.is_ok(), recovered);
        let success = run.is_ok()
            && death.as_ref().is_ok_and(|s| s.success())
            && policy.released
            && !transition_unresolved;
        let report = serde_json::json!({"nonce":c.nonce,"tag":tag,"y_evidence":policy.evidence(),"limits":limits,
        "complete_step":policy.complete,"transition_unresolved":transition_unresolved,"writer_death_confirmed":death.is_ok(),"unresolved":unresolved,
        "status":if success{"released-gesture-ui-unverified"}else if recovered{"failed-gesture-conditionally-cleaned"}
            else{"gesture-refused-or-unresolved"},"error":run.err().map(|e|e.to_string()),
        "ui_verified":false,"exclusive_kernel_ownership":false});
        ensure!(now()? < limits.overall, "Original overall response ceiling");
        durable(root, "gesture.result", &report)?;
        ensure!(
            now()? < limits.overall,
            "Result retention exceeded original ceiling"
        );
        println!("{report}");
        ensure!(success, "Gesture failed; never replay");
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn seed(y: i32) -> Seed {
            Seed {
                ids: [-1; 32],
                x: [0; 32],
                y: [y; 32],
                selected: 0,
            }
        }
        fn policy(y: i32) -> Policy {
            Policy::new(731, (0..16).map(|i| [100 + i * 20, 936]).collect(), seed(y)).unwrap()
        }
        fn state(p: &Policy) -> Seed {
            let mut s = p.seed.clone();
            s.selected = 0;
            s.ids[0] = if p.active { p.tag } else { -1 };
            s.x[0] = p.point[0];
            s.y[0] = p.point[1];
            s
        }
        fn down(p: &mut Policy) {
            let e = p.expected(0);
            p.admit(&e, &p.seed.clone()).unwrap();
            for v in e {
                p.observe(v).unwrap();
            }
        }
        fn limits() -> Limits {
            Limits {
                go: 100,
                write: 1600,
                recovery: 2600,
                overall: 5100,
            }
        }
        #[test]
        fn exact_sequence_and_suppressed_axes_release() {
            for y in [935, 936] {
                let mut p = policy(y);
                down(&mut p);
                assert_eq!(
                    p.evidence(),
                    if y == 936 {
                        "seed-carried"
                    } else {
                        "first-frame-echo"
                    }
                );
                for step in 1..16 {
                    let e = p.expected(step);
                    assert_eq!(e[1], [3, 57, 731]);
                    p.admit(&e, &state(&p)).unwrap();
                    // Kernel suppresses unchanged slot/tag/Y.
                    p.observe(e[2]).unwrap();
                    p.observe([0, 0, 0]).unwrap();
                }
                let e = p.expected(16);
                p.admit(&e, &state(&p)).unwrap();
                p.observe([3, 57, -1]).unwrap();
                p.observe([0, 0, 0]).unwrap();
                assert!(p.released && state(&p).released());
                assert!(p.observe([0, 0, 0]).is_err());
            }
        }
        #[test]
        fn first_syn_cannot_be_late_repaired() {
            for y in [935, 936] {
                let mut p = policy(y);
                p.admit(&p.expected(0), &p.seed.clone()).unwrap();
                p.observe([3, 57, 731]).unwrap();
                assert!(p.observe([0, 0, 0]).is_err());
            }
            let mut p = policy(935);
            p.admit(&p.expected(0), &p.seed.clone()).unwrap();
            p.observe([3, 57, 731]).unwrap();
            p.observe([3, 53, 100]).unwrap();
            assert!(p.observe([0, 0, 0]).is_err());
            let mut p = policy(936);
            p.admit(&p.expected(0), &p.seed.clone()).unwrap();
            p.observe([3, 57, 731]).unwrap();
            p.observe([3, 53, 100]).unwrap();
            p.observe([0, 0, 0]).unwrap();
            assert_eq!(p.complete, Some(0));
        }
        #[test]
        fn full_seed_and_failed_start_refuse_up() {
            let mut p = policy(936);
            let mut s = p.seed.clone();
            s.y[31] += 1;
            assert!(p.admit(&p.expected(0), &s).is_err());
            assert!(p.admit(&p.expected(16), &p.seed.clone()).is_err());
            assert!(!p.recovery_owned(&p.seed));
            assert!(Policy::new(731, p.path.clone(), seed(936)).is_ok());
            let mut same = seed(936);
            same.x[0] = 100;
            assert!(Policy::new(731, p.path.clone(), same).is_err());
        }
        #[test]
        fn exact_partial_prefix_is_required_and_never_path_membership() {
            let mut p = policy(935);
            p.admit(&p.expected(0), &p.seed.clone()).unwrap();
            p.observe([3, 57, 731]).unwrap();
            p.observe([3, 53, 100]).unwrap();
            assert!(p.recovery_owned(&state(&p))); // Y still seed935, actual retained partial prefix.
            let mut other = state(&p);
            other.y[0] = 936;
            assert!(!p.recovery_owned(&other));
            let mut p = policy(936);
            down(&mut p);
            let e = p.expected(1);
            p.admit(&e, &state(&p)).unwrap();
            assert!(p.recovery_owned(&state(&p)));
            p.observe(e[2]).unwrap();
            assert!(p.recovery_owned(&state(&p)));
            let mut later = state(&p);
            later.x[0] = p.path[2][0];
            assert!(!p.recovery_owned(&later));
            let mut foreign = state(&p);
            foreign.ids[4] = 9;
            assert!(!p.recovery_owned(&foreign));
            p.bad = true;
            assert!(!p.recovery_owned(&state(&p)));
        }
        #[test]
        fn duplicate_replay_wrong_tag_and_nonprefix_refuse() {
            let mut p = policy(936);
            down(&mut p);
            assert!(p.admit(&p.expected(0), &state(&p)).is_err());
            let e = p.expected(1);
            p.admit(&e, &state(&p)).unwrap();
            assert!(p.admit(&e, &state(&p)).is_err());
            assert!(p.observe([3, 57, 1]).is_err());
            let mut p = policy(936);
            p.admit(&p.expected(0), &p.seed.clone()).unwrap();
            p.observe([3, 53, 100]).unwrap();
            assert!(p.observe([3, 57, 731]).is_err());
            let mut p = policy(936);
            p.admit(&p.expected(0), &p.seed.clone()).unwrap();
            p.observe([3, 57, 731]).unwrap();
            assert!(p.observe([3, 57, 731]).is_err());
        }
        #[test]
        fn recovery_requires_dead_writer_original_cutoff_and_owned_prefix() {
            let mut p = policy(936);
            down(&mut p);
            let s = state(&p);
            let l = limits();
            assert!(recovery_admission(&p, &s, false, 200, &l).is_err());
            assert!(recovery_admission(&p, &s, true, 2600, &l).is_err());
            assert!(recovery_admission(&p, &s, true, 99, &l).is_err());
            assert!(recovery_admission(&p, &s, true, 2599, &l).is_ok());
            let mut foreign = s;
            foreign.ids[0] = 732;
            assert!(recovery_admission(&p, &foreign, true, 200, &l).is_err());
        }
        #[test]
        fn count_and_redirect_drift_are_bound() {
            let base = serde_json::json!({"pageCount":8,"cPages":{"pages":[
            {"id":"a","idx":{"value":"0"},"redir":{"value":0}},
            {"id":"b","idx":{"value":"1"}}],"lastOpened":{"value":"a"}}});
            let hash = page_map(&base).unwrap();
            let mut changed = base.clone();
            changed["cPages"]["lastOpened"]["value"] = serde_json::json!("b");
            assert_eq!(page_map(&changed).unwrap(), hash);
            changed["pageCount"] = serde_json::json!(9);
            assert!(page_map(&changed).is_err());
            let mut changed = base.clone();
            changed["cPages"]["pages"][0]["redir"]["value"] = serde_json::json!(7);
            assert_ne!(page_map(&changed).unwrap(), hash);
            let mut changed = base;
            changed["cPages"]["pages"][1]["redir"] = serde_json::Value::Null;
            assert_ne!(page_map(&changed).unwrap(), hash);
        }
        #[test]
        fn transient_input_uncertainty_and_foreign_event_during_up_intent_stay_refused() {
            for fault in ["held-path", "inventory", "pen-key", "query"] {
                let mut p = policy(936);
                down(&mut p);
                assert!(sticky_input::<()>(&mut p, Err(anyhow::anyhow!(fault))).is_err());
                sticky_input(&mut p, Ok(())).unwrap();
                assert!(!p.recovery_owned(&state(&p)));
            }
            let mut p = policy(936);
            down(&mut p);
            p.intent = Some(16);
            p.cursor = 0;
            let fault = p.observe([3, 57, 999]);
            sticky_input(&mut p, fault).unwrap_err();
            assert!(!p.recovery_owned(&state(&p)));
        }
        #[test]
        fn source_neighbor_straddle_only_allows_release_and_is_latched() {
            assert!(owner_admission(1, 0, 1, 1, true, false).unwrap());
            assert!(owner_admission(1, 0, 0, 0, true, false).is_err());
            assert!(owner_admission(1, 0, 1, 0, false, false).is_err());
            assert!(!owner_admission(1, 0, 1, 0, false, true).unwrap());
            assert!(owner_admission(1, 0, 7, 0, false, true).is_err());
            let mut p = policy(936);
            down(&mut p);
            let mut transition = false;
            accept_written(
                &mut p,
                &mut transition,
                &Written {
                    bytes: 48,
                    coherent: false,
                    input_valid: true,
                },
                48,
            )
            .unwrap();
            assert!(transition && !p.bad);
            accept_written(
                &mut p,
                &mut transition,
                &Written {
                    bytes: 48,
                    coherent: true,
                    input_valid: true,
                },
                48,
            )
            .unwrap();
            assert!(transition);
            assert!(accept_written(
                &mut p,
                &mut transition,
                &Written {
                    bytes: -1,
                    coherent: true,
                    input_valid: false
                },
                48
            )
            .is_err());
            assert!(p.bad);
        }
        #[test]
        fn expired_or_unconfirmed_writer_cannot_report_known_clear_contact() {
            assert!(contact_unresolved(false, true, false)); // Expired recovery, held contact.
            assert!(contact_unresolved(false, false, false));
            assert!(contact_unresolved(true, false, false));
            assert!(!contact_unresolved(false, true, true)); // Positive empty/conditional UP only.
            assert!(!contact_unresolved(true, true, false));
        }
        #[test]
        fn actual_stopped_and_hung_direct_children_are_reaped_before_release_authority() {
            for script in ["kill -STOP $$; exec sleep 30", "exec sleep 30"] {
                let mut child = Command::new("/bin/sh")
                    .arg("-c")
                    .arg(script)
                    .spawn()
                    .unwrap();
                let cutoff = now().unwrap() + 300;
                if script.starts_with("kill") {
                    let wait = now().unwrap() + 100;
                    while !stopped(&child).unwrap() && now().unwrap() < wait {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    assert!(stopped(&child).unwrap());
                }
                let status = fence(&mut child, cutoff, false).unwrap();
                assert!(!status.success());
                assert!(child.try_wait().unwrap().is_some());
                let mut p = policy(936);
                down(&mut p);
                let l = Limits {
                    go: cutoff - 300,
                    write: cutoff,
                    recovery: cutoff,
                    overall: cutoff + 500,
                };
                assert!(recovery_admission(&p, &state(&p), true, now().unwrap(), &l).is_ok());
            }
        }
    }

    pub fn main() -> Result<()> {
        ensure!(
            std::mem::size_of::<InputEvent>() == std::mem::size_of::<libc::input_event>(),
            "Input ABI mismatch"
        );
        #[cfg(target_pointer_width = "32")]
        ensure!(
            std::mem::size_of::<InputEvent>() == 16,
            "ARM input ABI mismatch"
        );
        let a = std::env::args().collect::<Vec<_>>();
        if a.len() == 3 && a[1] == "--page-map-sha256" {
            println!(
                "{}",
                page_map(&serde_json::from_slice(&bounded(Path::new(&a[2]), 65536)?)?)?
            );
            return Ok(());
        }
        ensure!(unsafe { libc::geteuid() } == 0, "Development root required");
        if a.len() == 2 {
            return supervisor(Path::new(&a[1]));
        }
        ensure!(
            a.len() == 6 && a[1] == "--worker",
            "Exact worker grammar required"
        );
        worker(
            Path::new(&a[2]),
            a[3].parse()?,
            serde_json::from_str(&a[4])?,
            a[5].parse()?,
        )
    }
}
#[cfg(all(target_os = "linux", feature = "development-input-diagnostics"))]
fn main() -> anyhow::Result<()> {
    owned::main()
}
#[cfg(not(all(target_os = "linux", feature = "development-input-diagnostics")))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("Development-input-diagnostics Linux example only")
}
