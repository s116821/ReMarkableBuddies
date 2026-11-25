//! Token access for one configured export. The same original grant remains
//! locked from admission, through any rotation, until the consumer is done.
use super::super::{SavedRevision, Secret};
use crate::{
    note_export::Failure,
    storage::{StorePaths, Uuid},
};
use std::time::Instant;
use tokio::sync::watch;

#[cfg(target_os = "linux")]
use super::{
    lifecycle::{now, GrantOwner, Phase},
    oauth::{Budget, GrantOperation, OAuth, Registration},
};
#[cfg(target_os = "linux")]
use std::time::Duration;

type Result<T> = std::result::Result<T, Failure>;

pub(crate) struct Access {
    #[cfg(target_os = "linux")]
    data: LinuxAccess,
}

#[cfg(target_os = "linux")]
struct LinuxAccess {
    owner: Option<GrantOwner>,
    saved: Option<SavedRevision>,
    space: Uuid,
    grant: Uuid,
    may_write: bool,
    registration: Option<Registration>,
    attempted_rotation: bool,
}

impl Access {
    pub(crate) fn open(
        paths: &StorePaths,
        saved: Option<SavedRevision>,
        space: Uuid,
        grant: Uuid,
        may_write: bool,
    ) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            Self::open_registered(
                paths,
                saved,
                space,
                grant,
                may_write,
                Registration::production(),
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (paths, saved, space, grant, may_write);
            Err(Failure::Unsupported)
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn open_registered(
        paths: &StorePaths,
        saved: Option<SavedRevision>,
        space: Uuid,
        grant: Uuid,
        may_write: bool,
        registration: Option<Registration>,
    ) -> Result<Self> {
        let owner = GrantOwner::open(&paths.credentials, grant)?;
        let _ = owner.active_grant(space, may_write)?;
        Ok(Self {
            data: LinuxAccess {
                owner: Some(owner),
                saved,
                space,
                grant,
                may_write,
                registration,
                attempted_rotation: false,
            },
        })
    }

    pub(crate) fn admit(&self, actor: Option<Uuid>, allow_rotation: bool) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let lease = &self.data;
            let owner = lease.owner.as_ref().ok_or(Failure::Authentication)?;
            let (grant, enrollment) = owner.active_grant(lease.space, lease.may_write)?;
            if let Some(actor) = actor {
                check_actor(enrollment.as_ref(), actor)?;
            }
            if grant.expires_at > now()? {
                return Ok(());
            }
            if allow_rotation
                && lease.saved.is_some()
                && lease.registration.as_ref().is_some_and(|approval| {
                    enrollment.is_some_and(|binding| binding.client_id == approval.client_id)
                })
            {
                return Ok(());
            }
            Err(Failure::Authentication)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (actor, allow_rotation);
            Err(Failure::Unsupported)
        }
    }

    pub(crate) fn admit_actor(&self, actor: Uuid) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let owner = self.data.owner.as_ref().ok_or(Failure::Authentication)?;
            check_actor(owner.status()?.enrollment.as_ref(), actor)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = actor;
            Err(Failure::Unsupported)
        }
    }

    pub(crate) fn config_current(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if self
            .data
            .saved
            .as_ref()
            .is_some_and(|saved| !saved.current())
        {
            return Err(Failure::Conflict);
        }
        Ok(())
    }

    pub(crate) async fn current(
        &mut self,
        actor: Uuid,
        deadline: Instant,
        canceled: watch::Receiver<bool>,
        selected: impl Fn() -> Result<()>,
    ) -> Result<Secret> {
        #[cfg(target_os = "linux")]
        {
            let lease = &mut self.data;
            let deadline = deadline.min(Instant::now() + Duration::from_secs(30));
            let budget = Budget::new(deadline, canceled);
            budget.current()?;
            selected()?;
            if lease.saved.as_ref().is_some_and(|saved| !saved.current()) {
                return Err(Failure::Conflict);
            }
            let owner = lease.owner.as_ref().ok_or(Failure::Authentication)?;
            let (grant, enrollment) = owner.active_grant(lease.space, lease.may_write)?;
            check_actor(enrollment.as_ref(), actor)?;
            if grant.expires_at > now()? {
                if lease.saved.as_ref().is_some_and(|saved| !saved.current()) {
                    return Err(Failure::Conflict);
                }
                budget.current()?;
                return Ok(Secret(grant.access_token));
            }
            let saved = lease.saved.as_ref().ok_or(Failure::Authentication)?;
            let approval = lease.registration.as_ref().ok_or(Failure::Authentication)?;
            let snapshot = owner.status()?;
            if lease.attempted_rotation
                || snapshot.phase != Phase::Active
                || !snapshot.enrollment.as_ref().is_some_and(|binding| {
                    binding.tablet_id == actor.to_string()
                        && binding.client_id == approval.client_id
                })
            {
                return Err(Failure::Authentication);
            }
            let unchanged = || {
                if !saved.current() {
                    return Err(Failure::Conflict);
                }
                selected()?;
                budget.current().map(|_| ())
            };
            let oauth = OAuth::new(approval)?;
            oauth
                .verify_metadata_for(&budget, Some(GrantOperation::Refresh))
                .await?;
            unchanged()?;
            let owner = lease.owner.take().ok_or(Failure::Authentication)?;
            lease.attempted_rotation = true;
            let mut permit = match owner.begin_rotation_retained(
                lease.space,
                Uuid::new_v4(),
                snapshot.revision.as_deref(),
            ) {
                Ok(permit) => permit,
                Err((owner, failure)) => {
                    lease.owner = Some(*owner);
                    return Err(failure);
                }
            };
            let result = async {
                let grant = oauth
                    .refresh(&mut permit, lease.grant, lease.space, &budget, unchanged)
                    .await?;
                let bytes = serde_json::to_vec(&grant).map_err(|_| Failure::Storage)?;
                unchanged()?;
                permit.install_in_place(&bytes)?;
                unchanged()?;
                let (stored, enrollment) =
                    permit.owner().active_grant(lease.space, lease.may_write)?;
                check_actor(enrollment.as_ref(), actor)?;
                if stored.expires_at <= now()? {
                    return Err(Failure::Authentication);
                }
                unchanged()?;
                Ok(Secret(stored.access_token))
            }
            .await;
            lease.owner = Some(permit.into_owner());
            result
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (actor, deadline, canceled, selected);
            Err(Failure::Unsupported)
        }
    }
}

#[cfg(target_os = "linux")]
fn check_actor(enrollment: Option<&super::lifecycle::Enrollment>, actor: Uuid) -> Result<()> {
    if enrollment.is_some_and(|binding| binding.tablet_id != actor.to_string()) {
        return Err(Failure::Authentication);
    }
    Ok(())
}
