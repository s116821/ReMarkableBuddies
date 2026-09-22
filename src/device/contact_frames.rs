//! Protocol-B frames for the history observer. Use raw evdev events: silently
//! reconstructed events after SYN_DROPPED cannot preserve edit ownership.
use super::interaction::Contact;

pub const MAX_SLOTS: usize = 64;

#[derive(Clone, Copy, Debug, Default)]
pub struct Slot {
    pub tracking: Option<i32>,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Observation {
    Pending,
    Frame(Vec<Contact>),
    Lost,
}

/// Coordinates remain native until the device-specific transform is applied.
/// Construction requires a kernel snapshot of every slot, including inactive
/// ones. A default zero-filled position is never accepted as observed input.
pub struct ContactFrames {
    slots: Vec<Slot>,
    selected: usize,
    lost: bool,
    committed: Vec<Contact>,
    pending: bool,
    reported_count: Option<usize>,
}

impl ContactFrames {
    pub fn seeded(slots: Vec<Slot>, selected: usize) -> Option<Self> {
        if slots.is_empty()
            || slots.len() > MAX_SLOTS
            || selected >= slots.len()
            || slots.iter().any(|s| {
                s.tracking.is_some_and(|id| id < 0)
                    || (s.tracking.is_some() && (s.x.is_none() || s.y.is_none()))
            })
        {
            return None;
        }
        let ids: Vec<_> = slots.iter().filter_map(|s| s.tracking).collect();
        if ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id)) {
            return None;
        }
        let mut result = Self {
            slots,
            selected,
            lost: false,
            committed: Vec::new(),
            pending: false,
            reported_count: None,
        };
        result.committed = result.snapshot()?;
        Some(result)
    }

    pub fn contacts(&self) -> Option<Vec<Contact>> {
        (!self.lost).then(|| self.committed.clone())
    }

    pub fn ready_for_timer(&self) -> bool {
        !self.lost && !self.pending
    }

    fn snapshot(&self) -> Option<Vec<Contact>> {
        if self.lost {
            return None;
        }
        let mut contacts = Vec::new();
        for (slot, state) in self.slots.iter().enumerate() {
            if let Some(tracking) = state.tracking {
                if contacts.iter().any(|c: &Contact| c.tracking == tracking) {
                    return None;
                }
                contacts.push(Contact {
                    slot: slot as u8,
                    tracking,
                    x: state.x?,
                    y: state.y?,
                });
            }
        }
        Some(contacts)
    }

    /// Losing a frame is sticky. Only a new observer with a fresh kernel
    /// snapshot may resume; the caller must discard the previous history.
    pub fn feed(&mut self, kind: u16, code: u16, value: i32) -> Observation {
        if self.lost {
            return Observation::Lost;
        }
        if kind != 0 {
            self.pending = true;
        }
        match (kind, code) {
            (1, 333 | 334 | 335 | 328) => {
                let count = match code {
                    333 => 2,
                    334 => 3,
                    335 => 4,
                    _ => 5,
                };
                match value {
                    1 => self.reported_count = Some(count),
                    0 if self.reported_count == Some(count) => self.reported_count = None,
                    0 => {}
                    _ => self.lost = true,
                }
            }
            (0, 3) => self.lost = true, // SYN_DROPPED
            (3, 47) => {
                if value < 0 || value as usize >= self.slots.len() {
                    self.lost = true;
                } else {
                    self.selected = value as usize;
                }
            }
            (3, 57) => {
                if value < -1 {
                    self.lost = true;
                } else {
                    self.slots[self.selected].tracking = (value >= 0).then_some(value);
                }
            }
            (3, 53) => self.slots[self.selected].x = Some(value),
            (3, 54) => self.slots[self.selected].y = Some(value),
            (0, 0) => {
                if let Some(contacts) = self.snapshot() {
                    if self
                        .reported_count
                        .is_some_and(|count| count != contacts.len())
                    {
                        self.lost = true;
                        return Observation::Lost;
                    }
                    self.committed = contacts.clone();
                    self.pending = false;
                    return Observation::Frame(contacts);
                }
                self.lost = true;
            }
            _ => {}
        }
        if self.lost {
            Observation::Lost
        } else {
            Observation::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_slots_are_reported_only_at_frame_boundary() {
        let mut decoder = ContactFrames::seeded(vec![Slot::default(); 8], 0).unwrap();
        for slot in [0, 3, 6, 7] {
            for (code, value) in [(47, slot), (57, 100 + slot), (53, 200 + slot), (54, 600)] {
                assert_eq!(decoder.feed(3, code, value), Observation::Pending);
            }
        }
        let Observation::Frame(contacts) = decoder.feed(0, 0, 0) else {
            panic!()
        };
        assert_eq!(
            contacts.iter().map(|c| c.slot).collect::<Vec<_>>(),
            [0, 3, 6, 7]
        );
        decoder.feed(3, 47, 3);
        decoder.feed(3, 57, -1);
        // A timer must continue to see the previous complete frame while a
        // partial release frame is arriving.
        assert_eq!(decoder.contacts().unwrap().len(), 4);
        assert!(!decoder.ready_for_timer());
        let Observation::Frame(contacts) = decoder.feed(0, 0, 0) else {
            panic!()
        };
        assert_eq!(contacts.len(), 3);
        assert!(decoder.ready_for_timer());
    }

    #[test]
    fn dropped_or_malformed_frames_cannot_recover_ownership() {
        for event in [(0, 3, 0), (3, 47, 16), (3, 47, -1), (3, 57, -2)] {
            let mut decoder = ContactFrames::seeded(vec![Slot::default(); 8], 0).unwrap();
            assert_eq!(decoder.feed(event.0, event.1, event.2), Observation::Lost);
            assert_eq!(decoder.feed(0, 0, 0), Observation::Lost);
            assert!(decoder.contacts().is_none());
        }
    }

    #[test]
    fn unknown_coordinates_and_duplicate_tracking_fail_closed() {
        let mut decoder = ContactFrames::seeded(vec![Slot::default(); 2], 0).unwrap();
        decoder.feed(3, 57, 123);
        assert_eq!(decoder.feed(0, 0, 0), Observation::Lost);
        let active = Slot {
            tracking: Some(123),
            x: Some(300),
            y: Some(500),
        };
        assert!(ContactFrames::seeded(vec![active, active], 0).is_none());
        let mut decoder = ContactFrames::seeded(vec![active, Slot::default()], 0).unwrap();
        for (code, value) in [(47, 1), (57, 123), (53, 500), (54, 600)] {
            decoder.feed(3, code, value);
        }
        assert_eq!(decoder.feed(0, 0, 0), Observation::Lost);
    }

    #[test]
    fn initial_kernel_snapshot_exposes_already_held_contacts() {
        let slots = vec![
            Slot::default(),
            Slot {
                tracking: Some(71),
                x: Some(12),
                y: Some(19),
            },
        ];
        let decoder = ContactFrames::seeded(slots, 1).unwrap();
        assert_eq!(
            decoder.contacts().unwrap(),
            vec![Contact {
                slot: 1,
                tracking: 71,
                x: 12,
                y: 19
            }]
        );
    }

    #[test]
    fn finger_count_hint_must_agree_with_reported_slots() {
        let active = Slot {
            tracking: Some(1),
            x: Some(200),
            y: Some(300),
        };
        let mut decoder = ContactFrames::seeded(
            vec![
                active,
                Slot {
                    tracking: Some(2),
                    ..active
                },
            ],
            0,
        )
        .unwrap();
        decoder.feed(1, 333, 1);
        assert!(matches!(decoder.feed(0, 0, 0), Observation::Frame(_)));
        decoder.feed(1, 335, 1);
        assert_eq!(decoder.feed(0, 0, 0), Observation::Lost);
    }
}
