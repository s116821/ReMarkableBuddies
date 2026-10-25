use super::*;
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};

pub(super) struct Fixture {
    root: PathBuf,
    baseline: GroupBaseline,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("buddy-supervisor-{}", Uuid::new_v4().simple()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let mount = root.join("cgroup");
        let slice = mount.join("system.slice");
        let group = slice.join("fixture.service");
        fs::create_dir_all(&group).unwrap();
        fs::write(group.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
        let context = Self::context_at(&mount).unwrap().identity;
        let baseline = GroupBaseline {
            mount_path: mount,
            unit: "fixture.service".into(),
            context,
            group: Identity::of(&fs::metadata(&group).unwrap()),
            parent: ProcessIdentity::current().unwrap(),
        };
        Self { root, baseline }
    }
    fn context_at(mount: &Path) -> Result<GroupContext> {
        // Real files, explicitly not a kernel cgroup fixture. Production's
        // fstatfs/mountinfo/membership checks are never disabled by public APIs.
        let directory = open_absolute_directory(mount)?;
        let slice = open_at(&directory, "system.slice", true, false)?;
        Ok(GroupContext {
            identity: ContextIdentity {
                boot: Uuid::parse_str(
                    text(Path::new("/proc/sys/kernel/random/boot_id"), 64)?.trim(),
                )?,
                mount_namespace: Identity::of(&fs::metadata("/proc/self/ns/mnt")?),
                cgroup_namespace: Identity::of(&fs::metadata("/proc/self/ns/cgroup")?),
                mount_record: format!("26 25 0:21 / {} rw - cgroup2 cgroup2 rw", mount.display()),
                mount: Identity::of(&directory.metadata()?),
                slice: Identity::of(&slice.metadata()?),
            },
            slice,
        })
    }
    fn observe(&self) -> Result<GroupObservation> {
        self.baseline.observe_with(Self::context_at)
    }
    fn group(&self) -> PathBuf {
        self.baseline
            .mount_path
            .join("system.slice/fixture.service")
    }
    pub(super) fn lease_dir(&self) -> PathBuf {
        let path = self.root.join("lease");
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }
    pub(super) fn record(&self) -> LeaseRecord {
        LeaseRecord::new(
            self.baseline.clone(),
            "23".repeat(16),
            1493716388120,
            StockBaseline {
                process: ProcessIdentity {
                    pid: 42,
                    start_ticks: 1234,
                },
                invocation: "12".repeat(16),
                executable_sha256: [1; 32],
                unit_sha256: [2; 32],
                dropin_sha256: [3; 32],
            },
            [4; 32],
            [5; 32],
            [6; 32],
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn original_population_and_deleted_final_component_are_distinct() {
    let f = Fixture::new();
    assert_eq!(f.observe().unwrap(), GroupObservation::Populated);
    fs::write(f.group().join("cgroup.events"), "populated 0\nfrozen 0\n").unwrap();
    assert_eq!(f.observe().unwrap(), GroupObservation::EmptyOriginal);
    fs::remove_dir_all(f.group()).unwrap();
    assert_eq!(f.observe().unwrap(), GroupObservation::OriginalAbsent);
    assert!(GroupBaseline::capture(&f.baseline.mount_path, "fixture.service").is_err());
}

#[test]
fn missing_events_or_parent_is_never_original_absent() {
    let f = Fixture::new();
    fs::remove_file(f.group().join("cgroup.events")).unwrap();
    assert!(f.observe().is_err());
    fs::remove_dir_all(f.baseline.mount_path.join("system.slice")).unwrap();
    assert!(f.observe().is_err());
}

#[test]
fn replaced_group_parent_and_symlink_refuse() {
    let f = Fixture::new();
    let moved = f.root.join("original");
    fs::rename(f.group(), &moved).unwrap();
    symlink(&moved, f.group()).unwrap();
    assert!(f.observe().is_err());
    fs::remove_file(f.group()).unwrap();
    fs::create_dir(f.group()).unwrap();
    fs::write(f.group().join("cgroup.events"), "populated 0\n").unwrap();
    assert!(f.observe().is_err());
    fs::rename(
        f.baseline.mount_path.join("system.slice"),
        f.root.join("old-slice"),
    )
    .unwrap();
    fs::create_dir(f.baseline.mount_path.join("system.slice")).unwrap();
    assert!(f.observe().is_err());
}

#[test]
fn malformed_duplicate_or_oversized_events_refuse() {
    let f = Fixture::new();
    for input in [
        "".into(),
        "frozen 0\n".into(),
        "populated 2\n".into(),
        "populated 0\npopulated 0\n".into(),
        "populated 0\nfrozen 0\nfrozen 1\n".into(),
        "populated 0\nunknown 0\n".into(),
        " ".repeat(4097),
    ] {
        fs::write(f.group().join("cgroup.events"), input).unwrap();
        assert!(f.observe().is_err());
    }
    fs::remove_file(f.group().join("cgroup.events")).unwrap();
    symlink("/dev/zero", f.group().join("cgroup.events")).unwrap();
    assert!(f.observe().is_err());
}

#[test]
fn context_and_path_changes_during_observation_refuse() {
    let f = Fixture::new();
    for index in 0..5 {
        let mut baseline = f.baseline.clone();
        match index {
            0 => baseline.context.boot = Uuid::new_v4(),
            1 => baseline.context.mount_namespace.inode += 1,
            2 => baseline.context.cgroup_namespace.inode += 1,
            3 => {
                baseline.context.mount_record =
                    baseline.context.mount_record.replacen("26 ", "27 ", 1)
            }
            _ => baseline.parent.start_ticks = 0,
        }
        assert!(baseline.observe_with(Fixture::context_at).is_err());
    }
    let calls = std::cell::Cell::new(0);
    let result = f.baseline.observe_with(|path| {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            fs::rename(f.group(), f.root.join("moved"))?;
        }
        Fixture::context_at(path)
    });
    assert!(result.is_err());
}

#[test]
fn lease_is_exclusive_and_recovery_reopen_does_not_consume_it() {
    let f = Fixture::new();
    let path = f.lease_dir();
    let record = f.record();
    let original = Lease::publish_inner(&path, record.clone()).unwrap();
    assert_eq!(original.record(), &record);
    assert_eq!(original.record().buddy_invocation(), "23".repeat(16));
    assert_eq!(original.record().buddy_start_micros(), 1493716388120);
    let bytes = fs::read(path.join(LEASE_NAME)).unwrap();
    assert!(Lease::publish_inner(&path, f.record()).is_err());
    for _ in 0..3 {
        let reopened = Lease::reopen(&path).unwrap();
        assert_eq!(reopened.record(), &record);
        reopened.current().unwrap();
    }
    assert_eq!(fs::read(path.join(LEASE_NAME)).unwrap(), bytes);
    // The public publisher cannot promote this ordinary-filesystem model.
    let other = f.root.join("other");
    fs::DirBuilder::new().mode(0o700).create(&other).unwrap();
    assert!(Lease::publish(&other, record).is_err());
    assert!(!other.join(LEASE_NAME).exists());
}

#[test]
fn partial_stale_unknown_and_duplicate_records_never_get_repaired() {
    let f = Fixture::new();
    let path = f.lease_dir();
    let record = f.record();
    let valid = serde_json::to_string(&record).unwrap();
    let mut stale = record.clone();
    stale.group.context.boot = Uuid::new_v4();
    for bytes in [
        Vec::new(),
        b"{\"version\":".to_vec(),
        valid.replacen('{', "{\"unknown\":0,", 1).into_bytes(),
        valid.replacen('{', "{\"version\":1,", 1).into_bytes(),
        serde_json::to_vec(&stale).unwrap(),
        vec![b' '; RECORD_LIMIT as usize + 1],
    ] {
        fs::write(path.join(LEASE_NAME), &bytes).unwrap();
        fs::set_permissions(path.join(LEASE_NAME), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Lease::reopen(&path).is_err());
        assert!(Lease::publish_inner(&path, record.clone()).is_err());
        assert_eq!(fs::read(path.join(LEASE_NAME)).unwrap(), bytes);
    }
}

#[test]
fn publication_failure_keeps_occupied_partial_record() {
    let f = Fixture::new();
    let path = f.lease_dir();
    let result = Lease::publish_using(&path, f.record(), |file, bytes| {
        file.write_all(&bytes[..12])?;
        bail!("injected publication failure")
    });
    assert!(result.is_err());
    let partial = fs::read(path.join(LEASE_NAME)).unwrap();
    assert_eq!(partial.len(), 12);
    assert!(Lease::reopen(&path).is_err());
    assert!(Lease::publish_inner(&path, f.record()).is_err());
    assert_eq!(fs::read(path.join(LEASE_NAME)).unwrap(), partial);
}

#[test]
fn lease_changes_links_and_directory_replacement_refuse() {
    let f = Fixture::new();
    let path = f.lease_dir();
    let lease = Lease::publish_inner(&path, f.record()).unwrap();
    let bytes = fs::read(path.join(LEASE_NAME)).unwrap();
    fs::write(path.join(LEASE_NAME), b"{}").unwrap();
    assert!(lease.current().is_err());
    fs::write(path.join(LEASE_NAME), &bytes).unwrap();
    fs::hard_link(path.join(LEASE_NAME), f.root.join("linked")).unwrap();
    assert!(lease.current().is_err());
    assert!(Lease::reopen(&path).is_err());
    fs::remove_file(f.root.join("linked")).unwrap();
    let old = f.root.join("old");
    fs::rename(&path, &old).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    fs::write(path.join(LEASE_NAME), &bytes).unwrap();
    assert!(lease.current().is_err());
    fs::remove_file(path.join(LEASE_NAME)).unwrap();
    symlink(old.join(LEASE_NAME), path.join(LEASE_NAME)).unwrap();
    assert!(Lease::reopen(&path).is_err());
}

#[test]
fn insecure_directory_and_symlink_ancestors_refuse() {
    let f = Fixture::new();
    let path = f.lease_dir();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Lease::publish_inner(&path, f.record()).is_err());
    assert!(!path.join(LEASE_NAME).exists());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = f.root.join("alias");
    symlink(&f.root, &alias).unwrap();
    assert!(Lease::publish_inner(&alias.join("lease"), f.record()).is_err());
    assert!(Lease::publish_inner(&f.root.join("lease/../lease"), f.record()).is_err());
}

#[test]
fn malformed_identity_cannot_become_deleted_group_evidence() {
    let f = Fixture::new();
    let path = f.lease_dir();
    fs::remove_dir_all(f.group()).unwrap();
    assert_eq!(f.observe().unwrap(), GroupObservation::OriginalAbsent);
    for index in 0..9 {
        let mut record = f.record();
        match index {
            0 => record.group.group.mode = 0,
            1 => record.group.group.mode = libc::S_IFREG | 0o755,
            2 => record.group.context.slice.mode = libc::S_IFREG | 0o755,
            3 => record.group.context.mount_namespace.mode = libc::S_IFDIR | 0o755,
            4 => record.group.group.device += 1,
            5 => record.group.context.slice.device += 1,
            6 => record.group.group.inode = record.group.context.slice.inode,
            7 => record.buddy_invocation.clear(),
            _ => record.buddy_invocation = "0".repeat(32),
        }
        let bytes = serde_json::to_vec(&record).unwrap();
        fs::write(path.join(LEASE_NAME), &bytes).unwrap();
        fs::set_permissions(path.join(LEASE_NAME), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Lease::reopen(&path).is_err(), "case {index}");
        if index < 7 {
            assert!(
                record.group.observe_with(Fixture::context_at).is_err(),
                "case {index}"
            );
        }
    }
}

#[test]
fn pinned_lease_refuses_identical_byte_replacement() {
    let f = Fixture::new();
    let path = f.lease_dir();
    let lease = Lease::publish_inner(&path, f.record()).unwrap();
    let bytes = fs::read(path.join(LEASE_NAME)).unwrap();
    fs::rename(path.join(LEASE_NAME), path.join("original")).unwrap();
    fs::write(path.join(LEASE_NAME), bytes).unwrap();
    fs::set_permissions(path.join(LEASE_NAME), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(lease.current().is_err());
}

#[test]
fn membership_requires_one_exact_unified_service_path() {
    validate_membership(
        "1:name=systemd:/system.slice/fixture.service\n0::/system.slice/fixture.service\n",
        "fixture.service",
    )
    .unwrap();
    for membership in [
        "",
        "0::/",
        "0::/system.slice/other.service\n",
        "0::/system.slice/fixture.service/child\n",
        "0::/system.slice/fixture.service\n0::/system.slice/fixture.service\n",
        "0:name=systemd:/system.slice/fixture.service\n",
    ] {
        assert!(validate_membership(membership, "fixture.service").is_err());
    }
}

#[test]
fn stale_membership_before_publication_creates_no_lease() {
    let f = Fixture::new();
    let path = f.lease_dir();
    assert!(Lease::publish_checked(&path, f.record(), |group| {
        validate_membership("0::/system.slice/other.service\n", group.unit())
    })
    .is_err());
    assert!(!path.join(LEASE_NAME).exists());
}

#[test]
fn changed_membership_after_publication_keeps_occupied_lease() {
    let f = Fixture::new();
    let path = f.lease_dir();
    let mut calls = 0;
    assert!(Lease::publish_checked(&path, f.record(), |group| {
        calls += 1;
        if calls == 2 {
            // The second check is after exclusive creation and synchronization.
            assert!(Lease::reopen(&path).is_ok());
            validate_membership("0::/system.slice/other.service\n", group.unit())
        } else {
            validate_membership("0::/system.slice/fixture.service\n", group.unit())
        }
    })
    .is_err());
    assert_eq!(calls, 2);
    let original = fs::read(path.join(LEASE_NAME)).unwrap();
    assert!(Lease::publish_inner(&path, f.record()).is_err());
    assert_eq!(fs::read(path.join(LEASE_NAME)).unwrap(), original);
}

#[test]
fn missing_or_zero_service_start_timestamp_is_refused() {
    let f = Fixture::new();
    let path = f.lease_dir();
    for missing in [false, true] {
        let mut value = serde_json::to_value(f.record()).unwrap();
        if missing {
            value.as_object_mut().unwrap().remove("buddy_start_micros");
        } else {
            value["buddy_start_micros"] = serde_json::json!(0);
        }
        fs::write(path.join(LEASE_NAME), serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(path.join(LEASE_NAME), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Lease::reopen(&path).is_err());
    }
}
