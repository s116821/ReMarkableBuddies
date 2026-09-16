//! Shared contact-frame policy. Native and simulated input feed complete SYN frames.
use super::touch::TriggerCorner;
use std::time::Duration;

const HOLD: Duration = Duration::from_secs(2);
const FORMATION: Duration = Duration::from_millis(250);
const RELEASE: Duration = Duration::from_millis(500);
const JITTER: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    pub slot: u8,
    pub tracking: i32,
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interaction {
    Reader,
    Undo,
    Redo,
    Invalidated,
}

struct Session {
    reference: Vec<Contact>,
    current: Vec<Contact>,
    since: Duration,
    invalidated: bool,
    blocked: bool,
    reader_sent: bool,
    qualified: Option<Interaction>,
    releasing: Option<Duration>,
}

pub struct ContactReducer {
    corner: TriggerCorner,
    session: Option<Session>,
}
impl ContactReducer {
    pub fn new(corner: TriggerCorner) -> Self {
        Self {
            corner,
            session: None,
        }
    }

    pub fn cancel(&mut self) -> Interaction {
        if let Some(session) = &mut self.session {
            session.blocked = true;
            session.invalidated = true;
        }
        Interaction::Invalidated
    }

    /// Must also be called on a timer with the last frame so stationary contacts
    /// qualify without requiring the hardware to send redundant position events.
    pub fn frame(&mut self, contacts: &[Contact], now: Duration) -> Vec<Interaction> {
        let mut contacts = contacts.to_vec();
        contacts.sort_by_key(|c| c.slot);
        let mut events = Vec::new();
        if contacts.is_empty() {
            if let Some(session) = self.session.take() {
                if !session.blocked
                    && !session.invalidated
                    && session
                        .releasing
                        .is_none_or(|start| now.saturating_sub(start) <= RELEASE)
                {
                    if let Some(action) = session.qualified {
                        events.push(action);
                    } else if !session.reader_sent {
                        events.push(Interaction::Invalidated);
                    }
                } else if !session.invalidated {
                    events.push(Interaction::Invalidated);
                }
            }
            return events;
        }
        let session = self.session.get_or_insert_with(|| Session {
            reference: contacts.clone(),
            current: contacts.clone(),
            since: now,
            invalidated: false,
            blocked: false,
            reader_sent: false,
            qualified: None,
            releasing: None,
        });
        let same_id = |a: &Contact, b: &Contact| a.slot == b.slot && a.tracking == b.tracking;
        let contains = |list: &[Contact], item: &Contact| list.iter().any(|old| same_id(old, item));
        let moved = contacts.iter().any(|contact| {
            session
                .reference
                .iter()
                .find(|old| same_id(old, contact))
                .is_some_and(|old| {
                    old.x.abs_diff(contact.x) > JITTER as u32
                        || old.y.abs_diff(contact.y) > JITTER as u32
                })
        });
        let duplicates = contacts.windows(2).any(|pair| pair[0].slot == pair[1].slot)
            || contacts
                .iter()
                .any(|c| c.tracking < 0 || usize::from(c.slot) >= super::contact_frames::MAX_SLOTS);
        let same = contacts.len() == session.current.len()
            && contacts
                .iter()
                .zip(&session.current)
                .all(|(a, b)| same_id(a, b));
        if moved || duplicates {
            session.blocked = true;
        }
        if !same {
            if session.qualified.is_some() {
                // After qualification, only stationary staggered lifts are valid.
                if contacts.iter().all(|c| contains(&session.current, c)) {
                    session.releasing.get_or_insert(now);
                } else {
                    session.blocked = true;
                }
            } else if session.current.iter().all(|c| contains(&contacts, c)) {
                session.reference = contacts.clone();
                session.since = now;
            } else {
                session.blocked = true;
            }
            session.current = contacts.clone();
        }
        if session
            .releasing
            .is_some_and(|start| now.saturating_sub(start) > RELEASE)
        {
            session.blocked = true;
        }
        let elapsed = now.saturating_sub(session.since);
        if !session.blocked
            && !session.reader_sent
            && contacts.len() == 1
            && self.corner.contains(contacts[0].x, contacts[0].y)
            && elapsed >= HOLD
        {
            session.reader_sent = true;
            events.push(Interaction::Reader);
        }
        if session.qualified.is_none()
            && !session.blocked
            && !session.invalidated
            && elapsed >= HOLD
        {
            session.qualified = match contacts.len() {
                2 => Some(Interaction::Redo),
                4 => Some(Interaction::Undo),
                _ => None,
            };
        }
        if !session.invalidated
            && (session.blocked
                || (session.qualified.is_none()
                    && !matches!(contacts.len(), 2 | 4)
                    && elapsed >= FORMATION))
        {
            session.invalidated = true;
            events.push(Interaction::Invalidated);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contacts(n: u8) -> Vec<Contact> {
        (0..n)
            .map(|slot| Contact {
                slot,
                tracking: 10 + i32::from(slot),
                x: 200 + i32::from(slot) * 70,
                y: 600,
            })
            .collect()
    }
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    #[test]
    fn stationary_holds_fire_only_after_full_release_and_rearm() {
        for (count, expected) in [(2, Interaction::Redo), (4, Interaction::Undo)] {
            let mut reducer = ContactReducer::new(TriggerCorner::LowerLeft);
            let points = contacts(count);
            assert!(reducer.frame(&points, ms(0)).is_empty());
            assert!(reducer.frame(&points, ms(1999)).is_empty());
            assert!(reducer.frame(&points, ms(2000)).is_empty());
            assert!(reducer.frame(&points, ms(4000)).is_empty());
            assert_eq!(reducer.frame(&[], ms(4001)), vec![expected]);
            assert!(reducer.frame(&[], ms(5000)).is_empty());
            assert!(reducer.frame(&points, ms(6000)).is_empty());
            assert_eq!(reducer.frame(&[], ms(6100)), vec![Interaction::Invalidated]);
        }
    }
    #[test]
    fn staggered_formation_and_lifts_do_not_emit_transient_two_contact_action() {
        let mut reducer = ContactReducer::new(TriggerCorner::LowerLeft);
        for count in 1..=4 {
            assert!(reducer
                .frame(&contacts(count), ms(u64::from(count) * 100))
                .is_empty());
        }
        assert!(reducer.frame(&contacts(4), ms(2400)).is_empty());
        let points = contacts(4);
        for released in 1..4 {
            assert!(reducer
                .frame(&points[released..], ms(2500 + released as u64 * 100))
                .is_empty());
        }
        assert_eq!(reducer.frame(&[], ms(2900)), vec![Interaction::Undo]);
    }
    #[test]
    fn motion_dropout_new_tracking_and_slow_single_contact_invalidate() {
        for variant in 0..4 {
            let mut reducer = ContactReducer::new(TriggerCorner::LowerLeft);
            let mut points = contacts(4);
            reducer.frame(&points, ms(0));
            match variant {
                0 => points[3].x += 20,
                1 => {
                    points.pop();
                }
                2 => points[2].tracking += 99,
                _ => {
                    points = contacts(1);
                }
            }
            assert_eq!(
                reducer.frame(&points, ms(1000)),
                vec![Interaction::Invalidated]
            );
            assert!(reducer.frame(&points, ms(4000)).is_empty());
            assert!(reducer.frame(&[], ms(4100)).is_empty());
        }
        let mut reducer = ContactReducer::new(TriggerCorner::LowerLeft);
        reducer.frame(&contacts(1), ms(0));
        assert_eq!(
            reducer.frame(&contacts(1), ms(250)),
            vec![Interaction::Invalidated]
        );
        reducer.frame(&contacts(2), ms(300));
        assert!(reducer.frame(&contacts(2), ms(3000)).is_empty());
        assert!(reducer.frame(&[], ms(3100)).is_empty());
    }
    #[test]
    fn corner_trigger_survives_history_invalidation_and_requires_release_to_repeat() {
        let mut reducer = ContactReducer::new(TriggerCorner::LowerLeft);
        let corner = [Contact {
            slot: 0,
            tracking: 1,
            x: 30,
            y: 970,
        }];
        reducer.frame(&corner, ms(0));
        assert_eq!(
            reducer.frame(&corner, ms(250)),
            vec![Interaction::Invalidated]
        );
        assert_eq!(reducer.frame(&corner, ms(2000)), vec![Interaction::Reader]);
        assert!(reducer.frame(&corner, ms(6000)).is_empty());
        assert!(reducer.frame(&[], ms(6001)).is_empty());
    }
    #[test]
    fn prolonged_partial_release_and_event_loss_never_mutate() {
        let mut reducer = ContactReducer::new(TriggerCorner::LowerLeft);
        reducer.frame(&contacts(4), ms(0));
        reducer.frame(&contacts(4), ms(2000));
        reducer.frame(&contacts(2), ms(2100));
        assert_eq!(
            reducer.frame(&contacts(2), ms(2700)),
            vec![Interaction::Invalidated]
        );
        assert!(reducer.frame(&[], ms(2800)).is_empty());
        reducer.frame(&contacts(2), ms(3000));
        assert_eq!(reducer.cancel(), Interaction::Invalidated);
        assert!(reducer.frame(&contacts(2), ms(6000)).is_empty());
        assert!(reducer.frame(&[], ms(6100)).is_empty());
    }
}
