//! Existing duplicated-descriptor regression, isolated from selected storage.
use super::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("buddy-owner-lease-{}", Uuid::new_v4())))
    }
    fn paths(&self) -> StorePaths {
        StorePaths {
            data: self.0.join("data"),
            cache: self.0.join("cache"),
            credentials: self.0.join("secrets"),
        }
    }
    fn store(&self) -> Store {
        Store::open(self.paths()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn inherited_descriptor_does_not_extend_a_dropped_owners_lease() {
    let fixture = Fixture::new();
    let store = fixture.store();
    // Same open-file description as the fork-to-exec window of another thread.
    let inherited = store._lease.try_clone().unwrap();
    drop(store);
    let reopened = fixture.store();
    drop(inherited);
    assert!(
        Store::open(fixture.paths()).is_err(),
        "dropping old descriptor cannot unlock the new owner"
    );
    drop(reopened);
    let _owner = fixture.store();
}
