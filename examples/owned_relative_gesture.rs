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
        geometry_variant: String,
        navigation_policy: String,
        virtual_path: Vec<[i32; 2]>,
        native_path: Vec<[i32; 2]>,
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
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GeometryEvidence {
        geometry_variant: String,
        navigation_policy: String,
        direction: String,
        virtual_path: Vec<[i32; 2]>,
        native_path: Vec<[i32; 2]>,
    }
    fn declared_paths(direction: &str) -> Result<(Vec<[i32; 2]>, Vec<[i32; 2]>)> {
        let (a, b) = match direction {
            "next" => (700i32, 110i32),
            "previous" => (100, 690),
            _ => bail!("Unknown direction"),
        };
        let touch = Touch::new(true, TriggerCorner::LowerLeft);
        let virtual_path = (0..=15)
            .map(|i| {
                let offset = (((b - a).abs() as f32) * (i as f32 / 15.0)) as i32;
                [a + (b - a).signum() * offset, 512]
            })
            .collect::<Vec<_>>();
        let native_path = virtual_path
            .iter()
            .map(|p| {
                let (x, y) = touch.diagnostic_native_point((p[0], p[1]));
                [x, y]
            })
            .collect();
        Ok((virtual_path, native_path))
    }
    fn geometry(c: &Config) -> GeometryEvidence {
        GeometryEvidence {
            geometry_variant: c.geometry_variant.clone(),
            navigation_policy: c.navigation_policy.clone(),
            direction: c.direction.clone(),
            virtual_path: c.virtual_path.clone(),
            native_path: c.native_path.clone(),
        }
    }
    fn geometry_selection(c: &Config) -> Result<()> {
        let (v, n) = declared_paths(&c.direction)?;
        ensure!(
            c.navigation_policy == "stop-at-declared-neighbor-v1"
                && c.geometry_variant == "fixed-gap10-v1"
                && c.virtual_path == v
                && c.native_path == n,
            "Missing/unknown/conflicting fixed geometry/path"
        );
        Ok(())
    }
    fn geometry_guard(c: &Config, root: &Path) -> Result<()> {
        let m = fs::symlink_metadata(root.join("gesture.path"))?;
        ensure!(
            m.is_file() && m.uid() == 0 && m.mode() & 0o7777 == 0o600 && m.nlink() == 1,
            "Private geometry evidence changed"
        );
        ensure!(
            serde_json::from_slice::<GeometryEvidence>(&bounded(
                &root.join("gesture.path"),
                16384
            )?)? == geometry(c),
            "Sealed geometry/path evidence changed"
        );
        Ok(())
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ContextSamples {
        first: [usize; 2],
        second: [usize; 2],
        bracket_changed: bool,
    }
    impl ContextSamples {
        fn coherent(&self) -> bool {
            !self.bracket_changed && self.first == self.second && self.first[0] == self.first[1]
        }
    }
    #[derive(Clone, Copy, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum DetectionOrigin {
        Parent,
        Worker,
    }
    #[derive(Clone, Copy, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum BoundaryGuard {
        BeforeIntent,
        AfterIntent,
        Context,
    }
    #[derive(Clone, Copy, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TargetObservation {
        samples: ContextSamples,
        witness_fields: [[bool; 2]; 2],
        worker_phase: Option<Phase>,
        worker_step: Option<usize>,
        origin: Option<DetectionOrigin>,
        guard_phase: Option<BoundaryGuard>,
        step: Option<usize>,
    }
    #[derive(Debug)]
    struct ExpectedTerminal(TargetObservation);
    impl std::fmt::Display for ExpectedTerminal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Declared neighbor observed; terminal release required")
        }
    }
    impl std::error::Error for ExpectedTerminal {}
    fn target_observation(samples: ContextSamples, neighbor: usize) -> Option<TargetObservation> {
        let witness_fields = [
            samples.first.map(|n| n == neighbor),
            samples.second.map(|n| n == neighbor),
        ];
        witness_fields
            .iter()
            .flatten()
            .any(|v| *v)
            .then_some(TargetObservation {
                samples,
                witness_fields,
                worker_phase: None,
                worker_step: None,
                origin: None,
                guard_phase: None,
                step: None,
            })
    }
    #[derive(Debug)]
    struct KnownStraddle(ContextSamples);
    impl std::fmt::Display for KnownStraddle {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "Declared-pair sampling transition {:?}->{:?}, bracket_changed={}; MOVE refused",
                self.0.first, self.0.second, self.0.bracket_changed
            )
        }
    }
    impl std::error::Error for KnownStraddle {}
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
            unsafe { libc::ioctl(fd, ior(0x40 + 47, std::mem::size_of::<Abs>()), &mut slot) } == 0
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
                    ior(0x40 + 47, std::mem::size_of::<Abs>()),
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
    fn tracking_range(fd: RawFd) -> Result<(i32, i32)> {
        let mut ids = [0i32; 6];
        ensure!(
            unsafe {
                libc::ioctl(
                    fd,
                    ior(0x40 + 57, std::mem::size_of_val(&ids)),
                    ids.as_mut_ptr(),
                )
            } == 0,
            "Tracking range unavailable"
        );
        let low = ids[1].max(1);
        let high = ids[2];
        ensure!(low <= high && high <= 65535, "Tracking range refused");
        Ok((low, high))
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
        geometry_selection(c)?;

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
    fn parse_context_sample(
        c: &Config,
        bytes: &[Vec<u8>],
        source_only: bool,
    ) -> Result<[usize; 2]> {
        ensure!(bytes.len() == 3, "Incomplete context sample");
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
        owner_admission(c.current_index, neighbor, ci, mi, source_only, true)?;
        if source_only {
            ensure!(
                bytes
                    .iter()
                    .enumerate()
                    .all(|(i, b)| sha(b) == c.sidecar_sha256[i]),
                "Pre-DOWN bytes changed"
            );
        }
        Ok([ci, mi])
    }
    fn classify_context_samples(
        c: &Config,
        first: &[Vec<u8>],
        second: &[Vec<u8>],
        source_only: bool,
        release_only: bool,
    ) -> Result<ContextSamples> {
        let a = parse_context_sample(c, first, source_only)?;
        let b = parse_context_sample(c, second, source_only)?;
        let samples = ContextSamples {
            first: a,
            second: b,
            bracket_changed: first != second,
        };
        if !source_only && !release_only {
            let neighbor = if c.direction == "next" {
                c.current_index + 1
            } else {
                c.current_index - 1
            };
            if let Some(observation) = target_observation(samples, neighbor) {
                return Err(ExpectedTerminal(observation).into());
            }
        }
        if !samples.coherent() && !release_only {
            return Err(KnownStraddle(samples).into());
        }
        Ok(samples)
    }
    fn parent_context_boundary(
        result: Result<ContextSamples>,
        step: usize,
        guard: BoundaryGuard,
    ) -> Result<ContextSamples> {
        match result {
            Err(e) if e.downcast_ref::<ExpectedTerminal>().is_some() => {
                let mut observation = e.downcast_ref::<ExpectedTerminal>().unwrap().0;
                observation.origin = Some(DetectionOrigin::Parent);
                observation.guard_phase = Some(guard);
                observation.step = Some(step);
                Err(ExpectedTerminal(observation).into())
            }
            other => other,
        }
    }
    fn session_guard(c: &Config) -> Result<()> {
        ensure!(
            fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim() == c.boot
                && start(c.stock_pid)? == c.stock_start,
            "Stock session changed"
        );
        Ok(())
    }
    fn context(
        c: &Config,
        root: &Path,
        source_only: bool,
        release_only: bool,
    ) -> Result<ContextSamples> {
        validate(c, root)?;
        geometry_guard(c, root)?;
        session_guard(c)?;
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
        let read = || {
            paths
                .iter()
                .map(|p| bounded(p, 65536))
                .collect::<Result<Vec<_>>>()
        };
        let first = read()?;
        let second = read()?;
        // Session/root/config/geometry remain sealed around both bounded samples.
        session_guard(c)?;
        validate(c, root)?;
        geometry_guard(c, root)?;
        classify_context_samples(c, &first, &second, source_only, release_only)
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
        c.native_path.clone()
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

    fn expected_release_eligible(
        c: &Config,
        target: Option<TargetObservation>,
        owned_prefix: bool,
        death: bool,
        released: bool,
        bad: bool,
        uncertain: bool,
    ) -> bool {
        c.navigation_policy == "stop-at-declared-neighbor-v1"
            && target.is_some()
            && owned_prefix
            && death
            && released
            && !bad
            && !uncertain
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
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        events: Vec<Event>,
        identity: Identity,
    }
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Permission {
        step: usize,
        write: bool,
        before: Seed,
        inventory: Vec<Identity>,
    }
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Written {
        bytes: isize,
        coherent: bool,
        input_valid: bool,
        context_samples: Option<ContextSamples>,
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
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum Phase {
        BeforeRequest,
        SendingRequest,
        AwaitPermission,
        PostPermission,
        Context,
        Input,
        PreWrite,
        Write,
        Reply,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum Cause {
        Protocol,
        Deadline,
        ContextUnknown,
        DeclaredStraddle,
        TargetObserved,
        InputUnknown,
        ShortWrite,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum ContextObserved {
        NotObserved,
        Coherent,
        DeclaredStraddle,
        Unknown,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum InputObserved {
        NotObserved,
        Valid,
        Unknown,
    }
    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct WorkerFault {
        step: usize,
        phase: Phase,
        cause: Cause,
        context: ContextObserved,
        input: InputObserved,
        bytes: Option<isize>,
        context_samples: Option<ContextSamples>,
        observed_ms: Option<i64>,
        diagnostic: String,
        diagnostic_truncated: bool,
    }
    struct Checked {
        phase: Phase,
        context: ContextObserved,
        input: InputObserved,
        bytes: Option<isize>,
        samples: Option<ContextSamples>,
    }
    impl Checked {
        fn fresh() -> Self {
            Self {
                phase: Phase::BeforeRequest,
                context: ContextObserved::NotObserved,
                input: InputObserved::NotObserved,
                bytes: None,
                samples: None,
            }
        }
        fn fault(&self, step: usize, cause: Cause, error: anyhow::Error) -> WorkerFault {
            let original = format!("{error:#}");
            let mut diagnostic = String::new();
            for ch in original.chars() {
                if diagnostic.len() + ch.len_utf8() > 512 {
                    break;
                }
                diagnostic.push(ch);
            }
            let diagnostic_truncated = diagnostic.len() != original.len();
            WorkerFault {
                step,
                phase: self.phase,
                cause,
                context: self.context,
                input: self.input,
                bytes: self.bytes,
                context_samples: self.samples,
                observed_ms: now().ok(),
                diagnostic,
                diagnostic_truncated,
            }
        }
    }
    type Attempt<T> = std::result::Result<T, (Cause, anyhow::Error)>;
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "phase", rename_all = "kebab-case", deny_unknown_fields)]
    enum WorkerFrame {
        Request { step: usize, request: Request },
        Written { step: usize, written: Written },
        Refused { fault: WorkerFault },
        Terminal { observation: WorkerFault },
    }
    struct Exchange {
        socket: UnixStream,
        limits: Limits,
        step: usize,
        terminal: Option<WorkerFault>,
    }
    impl Exchange {
        fn run(
            &mut self,
            events: &[InputEvent],
            identity: impl FnOnce() -> Result<Identity>,
            post: impl FnOnce(&Permission, &mut Checked) -> Attempt<Written>,
        ) -> Result<()> {
            ensure!(
                self.terminal.is_none(),
                "Terminal writer fault: no further request or write"
            );
            let mut observed = Checked::fresh();
            let step = self.step;
            let attempted = (|| -> Attempt<()> {
                self.limits.normal().map_err(|e| (Cause::Deadline, e))?;
                ensure_step(step).map_err(|e| (Cause::Protocol, e))?;
                let id = identity().map_err(|e| {
                    observed.input = InputObserved::Unknown;
                    (Cause::InputUnknown, e)
                })?;
                let e = events
                    .iter()
                    .map(|e| [e.event_type().0 as i32, e.code() as i32, e.value()])
                    .collect();
                observed.phase = Phase::SendingRequest;
                send(
                    &mut self.socket,
                    &WorkerFrame::Request {
                        step,
                        request: Request {
                            events: e,
                            identity: id,
                        },
                    },
                )
                .map_err(|e| (Cause::Protocol, e))?;
                observed.phase = Phase::AwaitPermission;
                let permission: Permission =
                    receive(&mut self.socket, self.limits.write).map_err(|e| {
                        (
                            if now().is_ok_and(|n| n >= self.limits.write) {
                                Cause::Deadline
                            } else {
                                Cause::Protocol
                            },
                            e,
                        )
                    })?;
                if permission.step != step || !permission.write {
                    return Err((
                        Cause::Protocol,
                        anyhow::anyhow!("Unexpected permission phase/step"),
                    ));
                }
                observed.phase = Phase::PostPermission;
                self.limits.normal().map_err(|e| (Cause::Deadline, e))?;
                let written = post(&permission, &mut observed)?;
                observed.bytes = Some(written.bytes);
                observed.phase = Phase::Write;
                if written.bytes != std::mem::size_of_val(events) as isize {
                    return Err((
                        Cause::ShortWrite,
                        anyhow::anyhow!("Partial/failed write; never replay"),
                    ));
                }
                observed.phase = Phase::Reply;
                send(&mut self.socket, &WorkerFrame::Written { step, written })
                    .map_err(|e| (Cause::Protocol, e))?;
                self.step += 1;
                Ok(())
            })();
            if let Err((cause, error)) = attempted {
                let fault = observed.fault(step, cause, error);
                // Latch BEFORE every reporting action. Caller STOP can never reenter the exchange.
                self.terminal = Some(fault.clone());
                if let Ok(b) = serde_json::to_vec(&fault) {
                    let mut line = b;
                    line.push(b'\n');
                    // One best-effort write to the private file held by the parent; no retries.
                    if now().is_ok_and(|n| n < self.limits.overall) {
                        unsafe {
                            libc::write(libc::STDERR_FILENO, line.as_ptr().cast(), line.len());
                        }
                    }
                }
                if let Ok(n) = now() {
                    let left = self.limits.overall - n;
                    if left > 0 {
                        let _ = self
                            .socket
                            .set_write_timeout(Some(Duration::from_millis((left as u64).min(5))));
                        let _ = send(
                            &mut self.socket,
                            &if fault.cause == Cause::TargetObserved {
                                WorkerFrame::Terminal {
                                    observation: fault.clone(),
                                }
                            } else {
                                WorkerFrame::Refused {
                                    fault: fault.clone(),
                                }
                            },
                        );
                    }
                }
                bail!(
                    "Worker {:?}/{:?} step{}: {}",
                    fault.phase,
                    fault.cause,
                    step,
                    fault.diagnostic
                )
            }
            Ok(())
        }
    }
    fn ensure_step(step: usize) -> Result<()> {
        ensure!(step <= 16, "Unknown gesture step");
        Ok(())
    }
    fn checked_context(result: Result<ContextSamples>, observed: &mut Checked) -> Attempt<()> {
        match result {
            Ok(samples) => {
                observed.samples = Some(samples);
                observed.context = if samples.coherent() {
                    ContextObserved::Coherent
                } else {
                    ContextObserved::DeclaredStraddle
                };
                Ok(())
            }
            Err(e) if e.downcast_ref::<ExpectedTerminal>().is_some() => {
                let samples = e.downcast_ref::<ExpectedTerminal>().unwrap().0.samples;
                observed.samples = Some(samples);
                observed.context = if samples.coherent() {
                    ContextObserved::Coherent
                } else {
                    ContextObserved::DeclaredStraddle
                };
                Err((Cause::TargetObserved, e))
            }
            Err(e) if e.downcast_ref::<KnownStraddle>().is_some() => {
                observed.samples = Some(e.downcast_ref::<KnownStraddle>().unwrap().0);
                observed.context = ContextObserved::DeclaredStraddle;
                Err((Cause::DeclaredStraddle, e))
            }
            Err(e) => {
                observed.context = ContextObserved::Unknown;
                Err((Cause::ContextUnknown, e))
            }
        }
    }
    fn sticky_context(
        policy: &mut Policy,
        transition: &mut bool,
        evidence: &mut Option<ContextSamples>,
        result: Result<ContextSamples>,
    ) -> Result<bool> {
        match result {
            Ok(samples) => {
                if !samples.coherent() {
                    *transition = true;
                    if evidence.is_none() {
                        *evidence = Some(samples);
                    }
                }
                Ok(samples.coherent())
            }
            Err(e) if e.downcast_ref::<ExpectedTerminal>().is_some() => {
                *transition = true;
                if evidence.is_none() {
                    *evidence = Some(e.downcast_ref::<ExpectedTerminal>().unwrap().0.samples);
                }
                Err(e)
            }
            Err(e) if e.downcast_ref::<KnownStraddle>().is_some() => {
                *transition = true;
                if evidence.is_none() {
                    *evidence = Some(e.downcast_ref::<KnownStraddle>().unwrap().0);
                }
                Err(e)
            }
            Err(e) => {
                policy.bad = true;
                Err(e)
            }
        }
    }
    fn validate_fault(f: &WorkerFault) -> Result<()> {
        ensure_step(f.step)?;
        ensure!(f.diagnostic.len() <= 512, "Fault diagnostic cap");
        let unseen = f.context == ContextObserved::NotObserved
            && f.input == InputObserved::NotObserved
            && f.bytes.is_none();
        let checked = matches!(
            f.context,
            ContextObserved::Coherent | ContextObserved::DeclaredStraddle
        ) && f.input == InputObserved::Valid;
        let expected = (if f.step == 0 {
            9
        } else if f.step == 16 {
            3
        } else {
            5
        }) * std::mem::size_of::<InputEvent>();
        let valid = match f.cause {
            Cause::TargetObserved => {
                f.step != 0
                    && f.phase == Phase::Context
                    && matches!(
                        f.context,
                        ContextObserved::Coherent | ContextObserved::DeclaredStraddle
                    )
                    && f.input == InputObserved::NotObserved
                    && f.bytes.is_none()
            }
            Cause::DeclaredStraddle => {
                f.step != 0
                    && f.phase == Phase::Context
                    && f.context == ContextObserved::DeclaredStraddle
                    && f.input == InputObserved::NotObserved
                    && f.bytes.is_none()
            }
            Cause::ContextUnknown => {
                f.phase == Phase::Context
                    && f.context == ContextObserved::Unknown
                    && f.input == InputObserved::NotObserved
                    && f.bytes.is_none()
            }
            Cause::InputUnknown => {
                f.input == InputObserved::Unknown
                    && f.bytes.is_none()
                    && (f.phase == Phase::BeforeRequest
                        && f.context == ContextObserved::NotObserved
                        || f.phase == Phase::Input
                            && matches!(
                                f.context,
                                ContextObserved::Coherent | ContextObserved::DeclaredStraddle
                            ))
            }
            Cause::ShortWrite => {
                f.phase == Phase::Write
                    && checked
                    && f.bytes.is_some_and(|n| n >= -1 && n < expected as isize)
            }
            Cause::Deadline => {
                matches!(
                    f.phase,
                    Phase::BeforeRequest | Phase::AwaitPermission | Phase::PostPermission
                ) && unseen
                    || f.phase == Phase::PreWrite && checked && f.bytes.is_none()
            }
            Cause::Protocol => {
                matches!(f.phase, Phase::SendingRequest | Phase::AwaitPermission) && unseen
                    || f.phase == Phase::PreWrite && checked && f.bytes.is_none()
                    || f.phase == Phase::Reply && checked && f.bytes == Some(expected as isize)
            }
        };
        let observed = match f.context {
            ContextObserved::NotObserved | ContextObserved::Unknown => f.context_samples.is_none(),
            ContextObserved::Coherent => f.context_samples.is_some_and(|s| s.coherent()),
            ContextObserved::DeclaredStraddle => f.context_samples.is_some_and(|s| !s.coherent()),
        };
        ensure!(valid && observed, "Contradictory worker fault observations");
        Ok(())
    }
    fn samples_scope(samples: ContextSamples, owners: [usize; 2], source_only: bool) -> Result<()> {
        for sample in [samples.first, samples.second] {
            owner_admission(
                owners[0],
                owners[1],
                sample[0],
                sample[1],
                source_only,
                true,
            )?;
        }
        Ok(())
    }
    fn check_frame(
        frame: WorkerFrame,
        step: usize,
        written_phase: bool,
        owners: [usize; 2],
        policy: &mut Policy,
        transition: &mut bool,
        primary: &mut Option<WorkerFault>,
    ) -> Result<WorkerFrame> {
        ensure_step(step)?;
        match &frame {
            WorkerFrame::Terminal { observation } => {
                validate_fault(observation)?;
                ensure!(
                    written_phase
                        && observation.step == step
                        && observation.cause == Cause::TargetObserved
                        && policy.complete.is_some()
                        && policy.first_syn
                        && !policy.bad
                        && !*transition
                        && primary.is_none(),
                    "Invalid terminal phase/step/owned DOWN"
                );
                let samples = observation
                    .context_samples
                    .context("Missing terminal samples")?;
                samples_scope(samples, owners, false)?;
                let mut target = target_observation(samples, owners[1])
                    .context("Forged source-only terminal")?;
                target.origin = Some(DetectionOrigin::Worker);
                target.guard_phase = Some(BoundaryGuard::Context);
                target.step = Some(observation.step);
                target.worker_phase = Some(observation.phase);
                target.worker_step = Some(observation.step);
                *transition = true;
                return Err(ExpectedTerminal(target).into());
            }
            WorkerFrame::Refused { fault } => {
                validate_fault(fault)?;
                ensure!(
                    fault.cause != Cause::TargetObserved,
                    "Target observation in refusal frame"
                );
                let phase_matches = if written_phase {
                    !matches!(fault.phase, Phase::BeforeRequest | Phase::SendingRequest)
                } else {
                    matches!(fault.phase, Phase::BeforeRequest | Phase::SendingRequest)
                };
                ensure!(phase_matches, "Refusal outside permitted exchange phase");
                if let Some(samples) = fault.context_samples {
                    samples_scope(samples, owners, step == 0)?;
                }
                ensure!(
                    fault.step == step && fault.diagnostic.len() <= 512 && primary.is_none(),
                    "Wrong/repeated worker refusal"
                );
                if fault.context == ContextObserved::DeclaredStraddle {
                    *transition = true;
                }
                if matches!(fault.cause, Cause::InputUnknown | Cause::ContextUnknown)
                    || fault.context == ContextObserved::Unknown
                    || fault.input == InputObserved::Unknown
                {
                    policy.bad = true;
                }
                *primary = Some(fault.clone());
                bail!(
                    "Worker {:?}/{:?} step{}: {}",
                    fault.phase,
                    fault.cause,
                    step,
                    fault.diagnostic
                )
            }
            WorkerFrame::Request { step: n, .. } => {
                ensure!(!written_phase && *n == step, "Wrong response phase/step")
            }
            WorkerFrame::Written { step: n, written } => {
                ensure!(written_phase && *n == step, "Wrong response phase/step");
                let samples = written
                    .context_samples
                    .context("Written context was not observed")?;
                samples_scope(samples, owners, step == 0)?;
                ensure!(
                    target_observation(samples, owners[1]).is_none(),
                    "Target observation in Written frame"
                );
                ensure!(
                    samples.coherent() || step == 16,
                    "Noncoherent MOVE response refused"
                );
                ensure!(
                    written.input_valid && written.coherent == samples.coherent(),
                    "Contradictory successful observations"
                );
            }
        }
        Ok(frame)
    }
    fn receive_worker(
        socket: &mut UnixStream,
        cutoff: i64,
        step: usize,
        written_phase: bool,
        root: &Path,
        config: &Config,
        responses: &mut File,
        policy: &mut Policy,
        transition: &mut bool,
        primary: &mut Option<WorkerFault>,
    ) -> Result<WorkerFrame> {
        let frame: WorkerFrame = receive(socket, cutoff)?;
        let b = serde_json::to_vec(
            &serde_json::json!({"expected_step":step,"await_written":written_phase,"frame":frame}),
        )?;
        ensure!(
            responses.metadata()?.len() + b.len() as u64 + 1 <= 32768,
            "Worker frame evidence cap"
        );
        responses.write_all(&b)?;
        responses.write_all(b"\n")?;
        let was_none = primary.is_none();
        let neighbor = if config.direction == "next" {
            config.current_index + 1
        } else {
            config.current_index - 1
        };
        let result = check_frame(
            frame,
            step,
            written_phase,
            [config.current_index, neighbor],
            policy,
            transition,
            primary,
        );
        if result.is_err() {
            responses.sync_all()?;
        }
        if was_none {
            if let Some(fault) = primary {
                durable(root, "gesture.worker-refusal", fault)?;
            }
        }
        result
    }
    struct Emitter {
        exchange: Exchange,
        c: Config,
        root: PathBuf,
        tag: i32,
        inputs: Inputs,
    }
    impl DiagnosticGestureEmitter for Emitter {
        fn emit(&mut self, device: &mut Device, events: &[InputEvent]) -> Result<()> {
            let fd = device.as_raw_fd();
            let c = &self.c;
            let root = &self.root;
            let inputs = &self.inputs;
            let tag = self.tag;
            let limits = self.exchange.limits.clone();
            self.exchange.run(
                events,
                || fd_identity(fd),
                |permission, observed| {
                    observed.phase = Phase::Context;
                    let start = events.len() == 9;
                    checked_context(context(c, root, start, false), observed)?;
                    observed.phase = Phase::Input;
                    let check = (|| -> Result<()> {
                        inputs.validate()?;
                        inputs.external_quiet()?;
                        ensure!(
                            inputs.identities() == permission.inventory
                                && fd_identity(fd)? == inputs.files[inputs.touch].2,
                            "Worker input binding drift"
                        );
                        ensure!(
                            snapshot(fd)? == permission.before,
                            "Postpermission contact drift"
                        );
                        Ok(())
                    })();
                    if let Err(e) = check {
                        observed.input = InputObserved::Unknown;
                        return Err((Cause::InputUnknown, e));
                    }
                    observed.input = InputObserved::Valid;
                    observed.phase = Phase::PreWrite;
                    ensure_tag(events[1].value(), tag).map_err(|e| (Cause::Protocol, e))?;
                    limits.normal().map_err(|e| (Cause::Deadline, e))?;
                    observed.phase = Phase::Write;
                    let n = unsafe {
                        libc::write(fd, events.as_ptr().cast(), std::mem::size_of_val(events))
                    };
                    observed.bytes = Some(n);
                    Ok(Written {
                        bytes: n,
                        coherent: observed.context == ContextObserved::Coherent,
                        input_valid: true,
                        context_samples: observed.samples,
                    })
                },
            )
        }
    }
    fn ensure_tag(value: i32, tag: i32) -> Result<()> {
        ensure!(value == -1 || value == tag, "Scoped tag changed");
        Ok(())
    }
    fn worker(root: &Path, tag: i32, limits: Limits, fd: RawFd) -> Result<()> {
        let c: Config = serde_json::from_slice(&bounded(&root.join("config.json"), 16384)?)?;
        validate(&c, root)?;
        geometry_guard(&c, root)?;
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
            exchange: Exchange {
                socket,
                limits,
                step: 0,
                terminal: None,
            },
            c: c.clone(),
            root: root.to_owned(),
            tag,
            inputs,
        };
        let mut touch = Touch::diagnostic_scoped_gesture(tag, Box::new(emitter))?;
        XochitlIntegration::swipe_fixed_gap10_development(
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
        durable(root, "gesture.path", &geometry(&c))?;
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
        let (low, high) = tracking_range(inputs.fd())?;
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
        let fault_file = exclusive(&root.join("worker-fault.log"))?;
        let retained_fault_file = fault_file.try_clone()?;
        let mut responses = exclusive(&root.join("worker-frames.jsonl"))?;
        let mut command = Command::new(executable);
        command
            .arg("--worker")
            .arg(root)
            .arg(tag.to_string())
            .arg(serde_json::to_string(&limits)?)
            .arg(fd.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(fault_file));
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
        let mut worker_refusal = None;
        let mut parent_context_transition = None;
        let mut protocol_uncertain = false;
        let mut parent_phase = "setup";
        let run = (|| -> Result<()> {
            setup?;
            for expected_step in 0..=16 {
                limits.normal()?;
                ensure!(!stopped(&child)?, "Stopped writer");
                parent_phase = "await-request";
                let received = receive_worker(
                    &mut socket,
                    limits.write,
                    expected_step,
                    false,
                    root,
                    &c,
                    &mut responses,
                    &mut policy,
                    &mut transition_unresolved,
                    &mut worker_refusal,
                );
                if received
                    .as_ref()
                    .is_err_and(|e| e.downcast_ref::<ExpectedTerminal>().is_none())
                    && worker_refusal.is_none()
                {
                    protocol_uncertain = true;
                }
                let frame = received?;
                let WorkerFrame::Request { request, .. } = frame else {
                    bail!("Expected request")
                };
                ensure!(
                    request.identity == inputs.files[inputs.touch].2,
                    "Writer descriptor differs"
                );
                inputs.drain(Some(&mut policy))?;
                sticky_input(&mut policy, inputs.validate())?;
                if !sticky_context(
                    &mut policy,
                    &mut transition_unresolved,
                    &mut parent_context_transition,
                    parent_context_boundary(
                        context(&c, root, expected_step == 0, false),
                        expected_step,
                        BoundaryGuard::BeforeIntent,
                    ),
                )? {
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
                if !sticky_context(
                    &mut policy,
                    &mut transition_unresolved,
                    &mut parent_context_transition,
                    parent_context_boundary(
                        context(&c, root, step == 0, false),
                        step,
                        BoundaryGuard::AfterIntent,
                    ),
                )? {
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
                        step,
                        write: true,
                        before,
                        inventory: inputs.identities(),
                    },
                )?;
                parent_phase = "await-written";
                let received = receive_worker(
                    &mut socket,
                    limits.write,
                    step,
                    true,
                    root,
                    &c,
                    &mut responses,
                    &mut policy,
                    &mut transition_unresolved,
                    &mut worker_refusal,
                );
                if received
                    .as_ref()
                    .is_err_and(|e| e.downcast_ref::<ExpectedTerminal>().is_none())
                    && worker_refusal.is_none()
                {
                    protocol_uncertain = true;
                }
                let frame = received?;
                let WorkerFrame::Written { written, .. } = frame else {
                    bail!("Expected written response")
                };
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
        let target_observation = run
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<ExpectedTerminal>())
            .map(|e| e.0);
        let last_motion_step = policy.complete;
        let owned_down_prefix = policy.complete.is_some()
            && policy.first_syn
            && policy.id_echo
            && policy.x_echo
            && !policy.bad;
        let death = fence(&mut child, limits.recovery, run.is_ok());
        let mut recovered = false;

        if death.is_ok() && run.is_err() && now()? < limits.recovery {
            let recovery = (|| -> Result<bool> {
                inputs.drain(Some(&mut policy))?;
                sticky_input(&mut policy, inputs.validate())?;
                if !sticky_context(
                    &mut policy,
                    &mut transition_unresolved,
                    &mut parent_context_transition,
                    context(&c, root, false, true),
                )? {
                    transition_unresolved = true;
                }
                let current = sticky_input(&mut policy, snapshot(inputs.fd()))?;
                if current.released() {
                    ensure!(
                        !policy.bad && now()? < limits.recovery,
                        "Released snapshot with sticky failure or expired cutoff"
                    );
                    if target_observation.is_some() {
                        ensure!(
                            owned_down_prefix
                                && current.x[0] == policy.point[0]
                                && current.y[0] == policy.point[1],
                            "Released snapshot lacks valid owned prefix"
                        );
                    }
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
                if !sticky_context(
                    &mut policy,
                    &mut transition_unresolved,
                    &mut parent_context_transition,
                    context(&c, root, false, true),
                )? {
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
        ensure!(now()? < limits.overall, "Original evidence cutoff");
        responses.sync_all()?;
        if death.is_ok() {
            ensure!(
                retained_fault_file.metadata()?.len() <= 8192,
                "Worker fault evidence cap"
            );
            retained_fault_file.sync_all()?;
        }
        let unresolved = contact_unresolved(run.is_ok(), death.is_ok(), recovered);
        let expected_released = expected_release_eligible(
            &c,
            target_observation,
            owned_down_prefix,
            death.is_ok(),
            recovered,
            policy.bad,
            protocol_uncertain,
        );
        let success = run.is_ok()
            && death.as_ref().is_ok_and(|s| s.success())
            && policy.released
            && !transition_unresolved;
        let report = serde_json::json!({"nonce":c.nonce,"geometry_variant":c.geometry_variant,"navigation_policy":c.navigation_policy,
        "trajectory_complete":success,"owned_down_prefix":owned_down_prefix,"release_positive":(success || recovered) && !policy.bad,
        "target_observation":target_observation.map(|o| serde_json::json!({"samples":o.samples,"witness_fields":o.witness_fields,"origin":o.origin,"phase":o.guard_phase,"step":o.step,"parent_waitstate":parent_phase,"worker_phase":o.worker_phase,"worker_step":o.worker_step})),"tag":tag,"y_evidence":policy.evidence(),"limits":limits,
        "complete_step":if expected_released { last_motion_step } else { policy.complete },"transition_unresolved":transition_unresolved,"writer_death_confirmed":death.is_ok(),"unresolved":unresolved,
        "status":if expected_released{"released-after-target-observation-ui-unverified"}else if success{"released-gesture-ui-unverified"}else if recovered{"failed-gesture-conditionally-cleaned"}
            else{"gesture-refused-or-unresolved"},"error":run.err().map(|e|e.to_string()),
        "parent_phase":parent_phase,"protocol_uncertain":protocol_uncertain,"worker_refusal":worker_refusal,"parent_context_transition":parent_context_transition,"ui_verified":false,"exclusive_kernel_ownership":false});
        ensure!(now()? < limits.overall, "Original overall response ceiling");
        durable(root, "gesture.result", &report)?;
        ensure!(
            now()? < limits.overall,
            "Result retention exceeded original ceiling"
        );
        println!("{report}");
        ensure!(success || expected_released, "Gesture failed; never replay");
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
        fn live_limits() -> Limits {
            let go = now().unwrap();
            Limits {
                go,
                write: go + 1500,
                recovery: go + 2500,
                overall: go + 5000,
            }
        }
        fn fixture_config() -> Config {
            let (v, n) = declared_paths("previous").unwrap();
            Config {
                nonce: "0".repeat(32),
                boot: "00000000-0000-0000-0000-000000000001".into(),
                stock_pid: 2,
                stock_start: 1,
                document: "00000000-0000-0000-0000-000000000009".into(),
                page_ids: (1..=8)
                    .map(|i| format!("00000000-0000-0000-0000-{i:012}"))
                    .collect(),
                current_index: 1,
                direction: "previous".into(),
                geometry_variant: "fixed-gap10-v1".into(),
                navigation_policy: "stop-at-declared-neighbor-v1".into(),
                virtual_path: v,
                native_path: n,
                root_dev: 0,
                root_ino: 0,
                source_observed_ms: 0,
                go_not_after_ms: 100,
                overall_ceiling_ms: 5100,
                source_image_sha256: "0".repeat(64),
                source_image_verified: true,
                sidecar_sha256: ["0".repeat(64), "0".repeat(64), "0".repeat(64)],
                page_map_sha256: "0".repeat(64),
                executable_sha256: "0".repeat(64),
            }
        }
        fn sample(c: &Config, ci: usize, mi: usize) -> Vec<Vec<u8>> {
            let content = serde_json::json!({"pageCount":8,"cPages":{"pages":c.page_ids.iter().enumerate().map(|(i,id)|{
                serde_json::json!({"id":id,"idx":{"value":i.to_string()},"redir":{"value":i}})
            }).collect::<Vec<_>>(),"lastOpened":{"value":c.page_ids[ci]}}});
            vec![
                format!("LastOpen=@ByteArray({})\n", c.document).into_bytes(),
                serde_json::to_vec(&content).unwrap(),
                serde_json::to_vec(&serde_json::json!({"lastOpenedPage":mi})).unwrap(),
            ]
        }
        fn sampled_config() -> (Config, Vec<Vec<u8>>) {
            let mut c = fixture_config();
            let b = sample(&c, 1, 1);
            c.page_map_sha256 = page_map(&serde_json::from_slice(&b[1]).unwrap()).unwrap();
            for i in 0..3 {
                c.sidecar_sha256[i] = sha(&b[i]);
            }
            (c, b)
        }
        fn owned_prefix(c: &Config) -> Policy {
            let mut p = Policy::new(731, c.native_path.clone(), seed(936)).unwrap();
            down(&mut p);
            for step in 1..=9 {
                let e = p.expected(step);
                p.admit(&e, &state(&p)).unwrap();
                for v in e {
                    p.observe(v).unwrap();
                }
            }
            p
        }
        fn fixture_root(label: &str) -> PathBuf {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let root = std::env::temp_dir()
                .join(format!("owned-gesture-{}-{label}-{n}", std::process::id()));
            fs::create_dir(&root).unwrap();
            root
        }
        fn fake_id() -> Identity {
            Identity {
                dev: 1,
                ino: 2,
                rdev: 3,
            }
        }
        #[test]
        fn fixed_geometry_is_explicit_and_sealed_with_inverse_endpoint_gaps() {
            let mut c = fixture_config();
            geometry_selection(&c).unwrap();
            let (pv, pn) = declared_paths("previous").unwrap();
            let (nv, nn) = declared_paths("next").unwrap();
            assert_eq!((pv[0], pv[15]), ([100, 512], [690, 512]));
            assert_eq!((nv[0], nv[15]), ([700, 512], [110, 512]));
            assert_eq!((pn[0], pn[15]), ([182, 936], [1261, 936]));
            assert_eq!((nn[0], nn[15]), ([1279, 936], [201, 936]));
            assert_eq!(nn[0][0] - pn[15][0], 18);
            assert_eq!(nn[15][0] - pn[0][0], 19);
            assert!(pn.windows(2).all(|p| p[0][0] < p[1][0]));
            assert!(nn.windows(2).all(|p| p[0][0] > p[1][0]));
            let mut json = serde_json::to_value(&c).unwrap();
            json.as_object_mut().unwrap().remove("geometry_variant");
            assert!(serde_json::from_value::<Config>(json).is_err());
            c.geometry_variant = "adaptive".into();
            assert!(geometry_selection(&c).is_err());
            c.geometry_variant = "fixed-gap10-v1".into();
            c.native_path[15][0] += 1;
            assert!(geometry_selection(&c).is_err());
            let c = fixture_config();
            let root = fixture_root("geometry");
            assert!(geometry_guard(&c, &root).is_err());
            durable(&root, "gesture.path", &geometry(&c)).unwrap();
            geometry_guard(&c, &root).unwrap();
            let mut changed = geometry(&c);
            changed.navigation_policy = "trajectory-v1".into();
            fs::write(
                root.join("gesture.path"),
                serde_json::to_vec(&changed).unwrap(),
            )
            .unwrap();
            assert!(geometry_guard(&c, &root).is_err());
            let mut changed = geometry(&c);
            changed.virtual_path[15][0] = 700;
            fs::write(
                root.join("gesture.path"),
                serde_json::to_vec(&changed).unwrap(),
            )
            .unwrap();
            assert!(geometry_guard(&c, &root).is_err());
            fs::remove_file(root.join("gesture.path")).unwrap();
            fs::remove_dir(root).unwrap();
        }
        #[test]
        fn two_existing_samples_distinguish_known_pair_transition_from_unknown() {
            let (c, first) = sampled_config();
            assert!(classify_context_samples(&c, &first, &first, true, false)
                .unwrap()
                .coherent());
            let neighbor = sample(&c, 0, 0);
            assert!(
                classify_context_samples(&c, &neighbor, &neighbor, false, false)
                    .unwrap_err()
                    .downcast_ref::<ExpectedTerminal>()
                    .is_some()
            );
            let straddle = sample(&c, 1, 0);
            for second in [&neighbor, &straddle] {
                let error = classify_context_samples(&c, &first, second, false, false).unwrap_err();
                let observed = error.downcast_ref::<ExpectedTerminal>().unwrap().0.samples;
                assert_eq!(observed.first, [1, 1]);
                assert_eq!(
                    observed.second,
                    if second == &neighbor { [0, 0] } else { [1, 0] }
                );
                assert!(observed.bracket_changed);
                assert!(!classify_context_samples(&c, &first, second, false, true)
                    .unwrap()
                    .coherent());
            }
            let mut timestamp = first.clone();
            let mut meta: serde_json::Value = serde_json::from_slice(&timestamp[2]).unwrap();
            meta["lastModified"] = serde_json::json!("new");
            timestamp[2] = serde_json::to_vec(&meta).unwrap();
            let error = classify_context_samples(&c, &first, &timestamp, false, false).unwrap_err();
            let observed = error.downcast_ref::<KnownStraddle>().unwrap().0;
            assert_eq!(observed.first, observed.second);
            assert!(observed.bracket_changed);
            // Before DOWN exact raw hashes remain mandatory for BOTH samples.
            assert!(classify_context_samples(&c, &first, &timestamp, true, true)
                .unwrap_err()
                .downcast_ref::<KnownStraddle>()
                .is_none());
            for fault in [
                "owner",
                "redir",
                "count",
                "LastOpen",
                "malformed",
                "missing",
            ] {
                let mut second = neighbor.clone();
                let mut content: serde_json::Value = serde_json::from_slice(&second[1]).unwrap();
                match fault {
                    "owner" => {
                        content["cPages"]["lastOpened"]["value"] = serde_json::json!(c.page_ids[7]);
                        second[1] = serde_json::to_vec(&content).unwrap();
                    }
                    "redir" => {
                        content["cPages"]["pages"][0]["redir"]["value"] = serde_json::json!(99);
                        second[1] = serde_json::to_vec(&content).unwrap();
                    }
                    "count" => {
                        content["pageCount"] = serde_json::json!(9);
                        second[1] = serde_json::to_vec(&content).unwrap();
                    }
                    "LastOpen" => second[0] = b"LastOpen=@ByteArray(foreign)\n".to_vec(),
                    "malformed" => second[1] = b"{bad".to_vec(),
                    _ => second[2] = b"{}".to_vec(),
                }
                let result = classify_context_samples(&c, &first, &second, false, true);
                assert!(result.is_err(), "{fault}");
                let mut p = owned_prefix(&c);
                let mut transition = false;
                let mut evidence = None;
                assert!(sticky_context(&mut p, &mut transition, &mut evidence, result).is_err());
                assert!(p.bad && !p.recovery_owned(&state(&p)));
            }
        }
        #[test]
        fn terminal_policy_requires_both_valid_samples_and_never_promotes_released_bad_state() {
            let (mut c, source) = sampled_config();
            let mut json = serde_json::to_value(&c).unwrap();
            json.as_object_mut().unwrap().remove("navigation_policy");
            assert!(serde_json::from_value::<Config>(json).is_err());
            c.navigation_policy = "unknown".into();
            assert!(geometry_selection(&c).is_err());
            c.navigation_policy = "stop-at-declared-neighbor-v1".into();
            let neighbor = sample(&c, 0, 0);
            for (first, second) in [
                (&neighbor, &neighbor),
                (&source, &neighbor),
                (&neighbor, &source),
                (&sample(&c, 1, 0), &sample(&c, 0, 1)),
            ] {
                let error = classify_context_samples(&c, first, second, false, false).unwrap_err();
                let target = error.downcast_ref::<ExpectedTerminal>().unwrap().0;
                assert!(target.witness_fields.iter().flatten().any(|v| *v));
                assert!(expected_release_eligible(
                    &c,
                    Some(target),
                    true,
                    true,
                    true,
                    false,
                    false
                ));
                for (prefix, death, released, bad, uncertain) in [
                    (false, true, true, false, false),
                    (true, false, true, false, false),
                    (true, true, false, false, false),
                    (true, true, true, true, false),
                    (true, true, true, false, true),
                ] {
                    assert!(!expected_release_eligible(
                        &c,
                        Some(target),
                        prefix,
                        death,
                        released,
                        bad,
                        uncertain
                    ));
                }
                assert!(!expected_release_eligible(
                    &c, None, true, true, true, false, false
                ));
                assert!(classify_context_samples(&c, first, second, true, false).is_err());
                let mut p = owned_prefix(&c);
                let mut transition = false;
                let mut evidence = None;
                let complete = p.complete;
                let error = sticky_context(
                    &mut p,
                    &mut transition,
                    &mut evidence,
                    classify_context_samples(&c, first, second, false, false),
                )
                .unwrap_err();
                assert!(error.downcast_ref::<ExpectedTerminal>().is_some());
                assert_eq!(p.complete, complete); // No intent/permission/write admission.
                assert!(transition && !p.bad);
            }
            for fault in [
                "owner",
                "redir",
                "count",
                "LastOpen",
                "malformed",
                "missing",
            ] {
                let mut second = source.clone();
                let mut content: serde_json::Value = serde_json::from_slice(&second[1]).unwrap();
                match fault {
                    "owner" => {
                        content["cPages"]["lastOpened"]["value"] = serde_json::json!("foreign");
                        second[1] = serde_json::to_vec(&content).unwrap();
                    }
                    "redir" => {
                        content["cPages"]["pages"][0]["redir"]["value"] = serde_json::json!(99);
                        second[1] = serde_json::to_vec(&content).unwrap();
                    }
                    "count" => {
                        content["pageCount"] = serde_json::json!(9);
                        second[1] = serde_json::to_vec(&content).unwrap();
                    }
                    "LastOpen" => second[0] = b"LastOpen=@ByteArray(foreign)\\n".to_vec(),
                    "malformed" => second[1] = b"{bad".to_vec(),
                    _ => second[2] = b"{}".to_vec(),
                }
                let mut p = owned_prefix(&c);
                let mut transition = false;
                let mut evidence = None;
                let error = sticky_context(
                    &mut p,
                    &mut transition,
                    &mut evidence,
                    classify_context_samples(&c, &neighbor, &second, false, false),
                )
                .unwrap_err();
                assert!(
                    error.downcast_ref::<ExpectedTerminal>().is_none(),
                    "{fault}"
                );
                assert!(p.bad && !p.recovery_owned(&state(&p)));
            }
        }
        #[test]
        fn target_at_normal_up_stops_before_intent_or_permission_with_truthful_parent_phase() {
            let (c, _) = sampled_config();
            let target = sample(&c, 0, 0);
            for guard in [BoundaryGuard::BeforeIntent, BoundaryGuard::AfterIntent] {
                let mut p = owned_prefix(&c);
                for step in 10..=15 {
                    let events = p.expected(step);
                    p.admit(&events, &state(&p)).unwrap();
                    for e in events {
                        p.observe(e).unwrap();
                    }
                }
                let before = p.complete;
                let intent_root = fixture_root("parent-up-intent");
                if matches!(guard, BoundaryGuard::AfterIntent) {
                    let events = p.expected(16);
                    p.admit(&events, &state(&p)).unwrap();
                    durable(
                        &intent_root,
                        "intent",
                        &serde_json::json!({"step":16,"events":events}),
                    )
                    .unwrap();
                }
                let mut transition = false;
                let mut evidence = None;
                let result = parent_context_boundary(
                    classify_context_samples(&c, &target, &target, false, false),
                    16,
                    guard,
                );
                let e = sticky_context(&mut p, &mut transition, &mut evidence, result).unwrap_err();
                let o = e.downcast_ref::<ExpectedTerminal>().unwrap().0;
                assert_eq!(o.step, Some(16));
                assert!(matches!(o.origin, Some(DetectionOrigin::Parent)));
                assert_eq!(
                    serde_json::to_value(o.guard_phase).unwrap(),
                    serde_json::to_value(guard).unwrap()
                );
                assert_eq!(p.complete, before);
                assert!(p.active && !p.released);
                assert_eq!(
                    p.intent == Some(16),
                    matches!(guard, BoundaryGuard::AfterIntent)
                );
                if matches!(guard, BoundaryGuard::AfterIntent) {
                    fs::remove_file(intent_root.join("intent")).unwrap();
                }
                fs::remove_dir(intent_root).unwrap();
                assert!(classify_context_samples(&c, &target, &target, false, true).is_ok());
            }
        }
        #[test]
        fn terminal_frames_reject_wrong_phase_step_forged_witness_and_missing_owned_down() {
            let c = fixture_config();
            let fault = WorkerFault {
                step: 10,
                phase: Phase::Context,
                cause: Cause::TargetObserved,
                context: ContextObserved::DeclaredStraddle,
                input: InputObserved::NotObserved,
                bytes: None,
                context_samples: Some(ContextSamples {
                    first: [1, 0],
                    second: [1, 0],
                    bracket_changed: false,
                }),
                observed_ms: Some(now().unwrap()),
                diagnostic: "target".into(),
                diagnostic_truncated: false,
            };
            for case in 0..7 {
                let mut f = fault.clone();
                let mut p = owned_prefix(&c);
                let mut transition = false;
                let mut primary = None;
                if case == 1 {
                    f.step = 9;
                }
                if case == 2 {
                    f.phase = Phase::Write;
                }
                if case == 3 {
                    f.context = ContextObserved::Coherent;
                    f.context_samples = Some(ContextSamples {
                        first: [1, 1],
                        second: [1, 1],
                        bracket_changed: false,
                    });
                }
                if case == 4 {
                    p.complete = None;
                    p.first_syn = false;
                }
                if case == 5 {
                    f.bytes = Some(80);
                }
                if case == 6 {
                    transition = true;
                }
                let result = check_frame(
                    WorkerFrame::Terminal { observation: f },
                    10,
                    case != 0,
                    [1, 0],
                    &mut p,
                    &mut transition,
                    &mut primary,
                );
                assert!(
                    result
                        .unwrap_err()
                        .downcast_ref::<ExpectedTerminal>()
                        .is_none(),
                    "case{case}"
                );
            }
        }
        #[test]
        #[ignore = "Internal direct-child protocol fixture, launched by the parent test only"]
        fn protocol_child_fixture() {
            let fd: RawFd = std::env::var("OWNED_PROTOCOL_FD").unwrap().parse().unwrap();
            let limits: Limits =
                serde_json::from_str(&std::env::var("OWNED_PROTOCOL_LIMITS").unwrap()).unwrap();
            let mode = std::env::var("OWNED_PROTOCOL_MODE").unwrap();
            let step = if mode == "broken-report" {
                0
            } else if mode == "target16" {
                16
            } else {
                10
            };
            let socket = unsafe { UnixStream::from_raw_fd(fd) };
            socket
                .set_write_timeout(Some(Duration::from_millis(5)))
                .unwrap();
            let mut client = Exchange {
                socket,
                limits,
                step,
                terminal: None,
            };
            let c = fixture_config();
            let p = Policy::new(731, c.native_path, seed(936)).unwrap();
            let events = p
                .expected(step)
                .into_iter()
                .map(|e| InputEvent::new(e[0] as u16, e[1] as u16, e[2]))
                .collect::<Vec<_>>();
            let result = client.run(
                &events,
                || Ok(fake_id()),
                |_, observed| {
                    assert_ne!(
                        mode, "broken-report",
                        "No permission exists on broken socket"
                    );
                    observed.phase = Phase::Context;
                    let samples = ContextSamples {
                        first: [1, 0],
                        second: [1, 0],
                        bracket_changed: false,
                    };
                    let error: anyhow::Error =
                        if mode == "target" || mode == "target16" || mode == "target-lost" {
                            ExpectedTerminal(target_observation(samples, 0).unwrap()).into()
                        } else {
                            KnownStraddle(samples).into()
                        };
                    checked_context(Err(error), observed)?;
                    unreachable!()
                },
            );
            assert!(result.is_err());
            assert!(client.terminal.is_some());
            let stop = p
                .expected(16)
                .into_iter()
                .map(|e| InputEvent::new(e[0] as u16, e[1] as u16, e[2]))
                .collect::<Vec<_>>();
            assert!(client
                .run(
                    &stop,
                    || panic!("Terminal STOP must not query/send"),
                    |_, _| panic!("Terminal STOP must not write")
                )
                .is_err());
        }
        fn spawn_protocol_fixture(
            root: &Path,
            limits: &Limits,
            mode: &str,
        ) -> (Child, Option<UnixStream>) {
            let (server, client) = UnixStream::pair().unwrap();
            let fd = client.as_raw_fd();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "owned::tests::protocol_child_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("OWNED_PROTOCOL_FD", fd.to_string())
                .env(
                    "OWNED_PROTOCOL_LIMITS",
                    serde_json::to_string(limits).unwrap(),
                )
                .env("OWNED_PROTOCOL_MODE", mode)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::from(
                    exclusive(&root.join("fault.log")).unwrap(),
                ));
            unsafe {
                command.pre_exec(move || {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let server = if mode == "broken-report" {
                drop(server);
                None
            } else {
                Some(server)
            };
            let child = command.spawn().unwrap();
            drop(client);
            (child, server)
        }
        #[test]
        fn actual_child_context_refusal_is_retained_then_fenced_before_conditional_release() {
            let c = fixture_config();
            let root = fixture_root("protocol");
            let limits = live_limits();
            let (mut child, socket) = spawn_protocol_fixture(&root, &limits, "context");
            let mut socket = socket.unwrap();
            let frame: WorkerFrame = receive(&mut socket, limits.write).unwrap();
            let WorkerFrame::Request { step, request } = frame else {
                panic!("Expected request")
            };
            assert_eq!(step, 10);
            let mut p = owned_prefix(&c);
            p.admit(&request.events, &state(&p)).unwrap();
            send(
                &mut socket,
                &Permission {
                    step: 10,
                    write: true,
                    before: state(&p),
                    inventory: vec![fake_id()],
                },
            )
            .unwrap();
            let mut transition = false;
            let mut primary = None;
            let mut frames = exclusive(&root.join("frames")).unwrap();
            let result = receive_worker(
                &mut socket,
                limits.write,
                10,
                true,
                &root,
                &c,
                &mut frames,
                &mut p,
                &mut transition,
                &mut primary,
            );
            assert!(result.unwrap_err().to_string().contains("DeclaredStraddle"));
            let retained: WorkerFault = serde_json::from_slice(
                &bounded(&root.join("gesture.worker-refusal"), 4096).unwrap(),
            )
            .unwrap();
            assert_eq!(retained.step, 10);
            assert_eq!(retained.phase, Phase::Context);
            assert_eq!(retained.cause, Cause::DeclaredStraddle);
            assert_eq!(retained.input, InputObserved::NotObserved);
            assert!(retained.bytes.is_none());
            assert!(transition && !p.bad);
            assert!(recovery_admission(&p, &state(&p), false, now().unwrap(), &limits).is_err());
            let death = fence(&mut child, limits.recovery, true).unwrap();
            assert!(death.success());
            // Child completed the terminal STOP assertion before successful process exit.
            assert!(receive::<WorkerFrame>(&mut socket, limits.recovery).is_err());
            recovery_admission(&p, &state(&p), true, now().unwrap(), &limits).unwrap();
            p.intent = Some(16);
            p.cursor = 0;
            for e in p.expected(16) {
                p.observe(e).unwrap();
            }
            assert!(p.released && state(&p).released());
            drop(frames);
            for n in ["fault.log", "frames", "gesture.worker-refusal"] {
                fs::remove_file(root.join(n)).unwrap();
            }
            fs::remove_dir(root).unwrap();
        }
        #[test]
        fn actual_child_target_terminal_stops_before_write_then_fences_before_release() {
            target_child_release("target");
            target_child_release("target16");
        }
        fn target_child_release(mode: &str) {
            let c = fixture_config();
            let root = fixture_root("target-protocol");
            let limits = live_limits();
            let (mut child, socket) = spawn_protocol_fixture(&root, &limits, mode);
            let mut socket = socket.unwrap();
            let frame: WorkerFrame = receive(&mut socket, limits.write).unwrap();
            let WorkerFrame::Request { step, request } = frame else {
                panic!("Expected request")
            };
            assert_eq!(step, if mode == "target16" { 16 } else { 10 });
            let mut p = owned_prefix(&c);
            for n in 10..step {
                let events = p.expected(n);
                p.admit(&events, &state(&p)).unwrap();
                for e in events {
                    p.observe(e).unwrap();
                }
            }
            p.admit(&request.events, &state(&p)).unwrap();
            send(
                &mut socket,
                &Permission {
                    step,
                    write: true,
                    before: state(&p),
                    inventory: vec![fake_id()],
                },
            )
            .unwrap();
            let mut transition = false;
            let mut primary = None;
            let mut frames = exclusive(&root.join("frames")).unwrap();
            let result = receive_worker(
                &mut socket,
                limits.write,
                step,
                true,
                &root,
                &c,
                &mut frames,
                &mut p,
                &mut transition,
                &mut primary,
            );
            let error = result.unwrap_err();
            let observation = error.downcast_ref::<ExpectedTerminal>().unwrap().0;
            assert_eq!(observation.samples.first, [1, 0]);
            assert!(primary.is_none() && !root.join("gesture.worker-refusal").exists());
            assert!(transition && !p.bad);
            assert!(recovery_admission(&p, &state(&p), false, now().unwrap(), &limits).is_err());
            let death = fence(&mut child, limits.recovery, true).unwrap();
            assert!(death.success());
            // Child completed the terminal STOP assertion before successful process exit.
            assert!(receive::<WorkerFrame>(&mut socket, limits.recovery).is_err());
            recovery_admission(&p, &state(&p), true, now().unwrap(), &limits).unwrap();
            p.intent = Some(16);
            p.cursor = 0;
            for e in p.expected(16) {
                p.observe(e).unwrap();
            }
            assert!(p.released && state(&p).released());
            drop(frames);
            for n in ["fault.log", "frames"] {
                fs::remove_file(root.join(n)).unwrap();
            }
            fs::remove_dir(root).unwrap();
        }
        #[test]
        fn actual_target_report_loss_after_permission_keeps_latch_and_cannot_earn_expected_release()
        {
            let c = fixture_config();
            let root = fixture_root("target-lost");
            let limits = live_limits();
            let (mut child, socket) = spawn_protocol_fixture(&root, &limits, "target-lost");
            let mut socket = socket.unwrap();
            let frame: WorkerFrame = receive(&mut socket, limits.write).unwrap();
            let WorkerFrame::Request { step, .. } = frame else {
                panic!("request required");
            };
            let p = owned_prefix(&c);
            send(
                &mut socket,
                &Permission {
                    step,
                    write: true,
                    before: state(&p),
                    inventory: vec![fake_id()],
                },
            )
            .unwrap();
            drop(socket); // Permission bytes queued; Terminal reply has no receiver.
            let death = fence(&mut child, limits.recovery, true).unwrap();
            assert!(death.success()); // Child verified terminal fallback STOP made zero calls.
            let fault: WorkerFault =
                serde_json::from_slice(&bounded(&root.join("fault.log"), 4096).unwrap()).unwrap();
            assert_eq!(fault.cause, Cause::TargetObserved);
            assert_eq!(fault.phase, Phase::Context);
            assert_eq!(fault.step, 10);
            assert!(fault.bytes.is_none() && fault.context_samples.is_some());
            let protocol_uncertain = true; // No complete Terminal frame was received.
            assert!(!expected_release_eligible(
                &c,
                None,
                true,
                true,
                true,
                false,
                protocol_uncertain
            ));
            fs::remove_file(root.join("fault.log")).unwrap();
            fs::remove_dir(root).unwrap();
        }
        #[test]
        fn failed_reporting_keeps_terminal_latch_and_private_primary_fault() {
            let root = fixture_root("broken");
            let limits = live_limits();
            let (mut child, socket) = spawn_protocol_fixture(&root, &limits, "broken-report");
            assert!(socket.is_none());
            let death = fence(&mut child, limits.recovery, true).unwrap();
            assert!(death.success());
            let fault: WorkerFault =
                serde_json::from_slice(&bounded(&root.join("fault.log"), 4096).unwrap()).unwrap();
            assert_eq!(fault.step, 0);
            assert_eq!(fault.phase, Phase::SendingRequest);
            assert_eq!(fault.cause, Cause::Protocol);
            assert_eq!(fault.context, ContextObserved::NotObserved);
            assert_eq!(fault.input, InputObserved::NotObserved);
            assert!(fault.context_samples.is_none() && fault.bytes.is_none());
            fs::remove_file(root.join("fault.log")).unwrap();
            fs::remove_dir(root).unwrap();
        }
        #[test]
        fn wrong_phase_step_observations_and_duplicate_refusals_are_rejected() {
            let c = fixture_config();
            let mut p = owned_prefix(&c);
            let mut transition = false;
            let mut primary = None;
            let request = WorkerFrame::Request {
                step: 10,
                request: Request {
                    events: p.expected(10),
                    identity: fake_id(),
                },
            };
            assert!(check_frame(
                request,
                10,
                true,
                [1, 0],
                &mut p,
                &mut transition,
                &mut primary
            )
            .is_err());
            let make_written = |step| WorkerFrame::Written {
                step,
                written: Written {
                    bytes: 80,
                    coherent: true,
                    input_valid: true,
                    context_samples: Some(ContextSamples {
                        first: [1, 1],
                        second: [1, 1],
                        bracket_changed: false,
                    }),
                },
            };
            assert!(check_frame(
                make_written(9),
                10,
                true,
                [1, 0],
                &mut p,
                &mut transition,
                &mut primary
            )
            .is_err());
            assert!(check_frame(
                make_written(10),
                10,
                false,
                [1, 0],
                &mut p,
                &mut transition,
                &mut primary
            )
            .is_err());
            let mut json = serde_json::to_value(make_written(10)).unwrap();
            json["written"]["coherent"] = serde_json::json!(false);
            json["written"]["context_samples"] =
                serde_json::json!({"first":[1,0],"second":[1,0],"bracket_changed":false});
            let decoded: WorkerFrame = serde_json::from_value(json).unwrap();
            assert!(check_frame(
                decoded,
                10,
                true,
                [1, 0],
                &mut p,
                &mut transition,
                &mut primary
            )
            .unwrap_err()
            .to_string()
            .contains("Target observation in Written"));
            let mut observed = Checked::fresh();
            observed.phase = Phase::Context;
            let fault = checked_context(Err(anyhow::anyhow!("Unknown document")), &mut observed)
                .unwrap_err();
            let fault = observed.fault(10, fault.0, fault.1);
            assert!(check_frame(
                WorkerFrame::Refused {
                    fault: fault.clone()
                },
                10,
                true,
                [1, 0],
                &mut p,
                &mut transition,
                &mut primary
            )
            .is_err());
            assert!(p.bad && !p.recovery_owned(&state(&p)));
            assert!(primary.is_some());
            assert!(check_frame(
                WorkerFrame::Refused { fault },
                10,
                true,
                [1, 0],
                &mut p,
                &mut transition,
                &mut primary
            )
            .unwrap_err()
            .to_string()
            .contains("repeated"));
            let mut json = serde_json::to_value(make_written(10)).unwrap();
            json["unexpected"] = serde_json::json!(true);
            assert!(serde_json::from_value::<WorkerFrame>(json).is_err());
        }
        #[test]
        fn refusal_stage_observations_never_default_to_valid() {
            let mut at = Checked::fresh();
            at.phase = Phase::BeforeRequest;
            let mut fault = at.fault(10, Cause::Deadline, anyhow::anyhow!("cutoff"));
            validate_fault(&fault).unwrap();
            fault.context = ContextObserved::Coherent;
            fault.context_samples = Some(ContextSamples {
                first: [1, 1],
                second: [1, 1],
                bracket_changed: false,
            });
            assert!(validate_fault(&fault).is_err());
            let mut at = Checked::fresh();
            at.phase = Phase::Reply;
            let mut fault = at.fault(10, Cause::Protocol, anyhow::anyhow!("lost response"));
            assert!(validate_fault(&fault).is_err());
            fault.context = ContextObserved::Coherent;
            fault.context_samples = Some(ContextSamples {
                first: [1, 1],
                second: [1, 1],
                bracket_changed: false,
            });
            fault.input = InputObserved::Valid;
            fault.bytes = Some(5 * std::mem::size_of::<InputEvent>() as isize);
            validate_fault(&fault).unwrap();
            fault.phase = Phase::Write;
            fault.cause = Cause::ShortWrite;
            assert!(validate_fault(&fault).is_err());
            fault.bytes = Some(-1);
            validate_fault(&fault).unwrap();
        }
        #[test]
        #[ignore = "Requires isolated C-header ioctl fixture; see examples/fixtures/owned_gesture_ioctl_fixture.c"]
        fn actual_snapshot_and_tracking_requests_match_linux_header() {
            let reset = unsafe {
                libc::dlsym(libc::RTLD_DEFAULT, b"owned_fixture_reset\0".as_ptr().cast())
            };
            assert!(
                !reset.is_null(),
                "Run with the compiled isolated C-header fixture in LD_PRELOAD"
            );
            let reset: unsafe extern "C" fn(i32) = unsafe { std::mem::transmute(reset) };
            unsafe { reset(0) };
            let s = snapshot(-137).unwrap();
            assert_eq!(s.selected, 0);
            assert!(s.released());
            for i in 0..32 {
                assert_eq!(s.x[i], 100 + i as i32);
                assert_eq!(s.y[i], 936 + i as i32);
            }
            assert_eq!(tracking_range(-137).unwrap(), (1, 65535));
            // Authoritative C header rejects the old slot/range request numbers.
            let mut a = [0i32; 6];
            for axis in [47, 57] {
                assert_eq!(
                    unsafe { libc::ioctl(-137, ior(0x20 + axis, 24), a.as_mut_ptr()) },
                    -1
                );
            }
            for (mode, error) in [
                (1, "Unknown MT layout"),
                (2, "Unknown MT layout"),
                (3, "Selected slot changed during query"),
                (4, "Contact changed during query"),
            ] {
                unsafe { reset(mode) };
                assert_eq!(snapshot(-137).unwrap_err().to_string(), error);
            }
            unsafe { reset(0) };
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
                    context_samples: None,
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
                    context_samples: None,
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
                    input_valid: false,
                    context_samples: None
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
        if a.len() == 3 && a[1] == "--geometry-paths" {
            let (v, n) = declared_paths(&a[2])?;
            println!(
                "{}",
                serde_json::to_string(&GeometryEvidence {
                    geometry_variant: "fixed-gap10-v1".into(),
                    navigation_policy: "stop-at-declared-neighbor-v1".into(),
                    direction: a[2].clone(),
                    virtual_path: v,
                    native_path: n
                })?
            );
            return Ok(());
        }
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
