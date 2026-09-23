//! Composite destination readiness for the verified RM2 layout. Metadata alone
//! is not visual proof; this is not a native rendering acknowledgement.
use super::{backend::NavigationCompletion, native_page::NavigationMetadata};
use crate::{workflow::xochitl_integration::NavigationDirection, Workflow};
use anyhow::{ensure, Context, Result};
use image::{DynamicImage, GrayImage};
use std::time::Duration;

#[derive(Clone)]
pub(super) struct Frame {
    pub before: NavigationMetadata,
    pub after: NavigationMetadata,
    pub image: GrayImage,
}

pub(super) trait NavigationIo {
    fn observe(&mut self) -> Result<Frame>;
    fn swipe(&mut self, direction: NavigationDirection) -> Result<()>;
    fn now(&self) -> Duration;
    fn pace(&mut self, duration: Duration);
    fn begin_guard(&mut self) -> Result<()>;
    fn guard(&mut self) -> Result<()>;
    fn end_guard(&mut self);
}

fn chrome_ready(image: &GrayImage) -> bool {
    super::status_readiness::footer_ready(image)
        && (61..984).all(|y| (735..740).all(|x| image.get_pixel(x, y)[0] >= 248))
}

fn same_pixels(a: &GrayImage, b: &GrayImage) -> bool {
    a.dimensions() == b.dimensions()
        && a.as_raw()
            .iter()
            .zip(b.as_raw())
            .all(|(x, y)| x.abs_diff(*y) <= 8)
}

fn check_metadata(
    current: &NavigationMetadata,
    source: &NavigationMetadata,
    target: &str,
    target_visit: &mut Option<String>,
) -> Result<()> {
    ensure!(
        current.owner.document == source.owner.document
            && current.owner.session == source.owner.session,
        "Navigation document/session changed"
    );
    ensure!(
        current.order == source.order,
        "Navigation page order or redirect changed"
    );
    if current.owner.page == source.owner.page {
        ensure!(
            current.owner.visit == source.owner.visit && target_visit.is_none(),
            "Navigation returned to a stale or different source visit"
        );
    } else {
        ensure!(
            current.owner.page == target && current.owner.visit != source.owner.visit,
            "Navigation reached wrong neighbor or stale target visit"
        );
        if let Some(visit) = target_visit {
            ensure!(
                *visit == current.owner.visit,
                "Navigation target visit changed"
            );
        } else {
            *target_visit = Some(current.owner.visit.clone());
        }
    }
    Ok(())
}

pub(super) fn navigate(
    io: &mut impl NavigationIo,
    direction: NavigationDirection,
) -> Result<NavigationCompletion> {
    let _timing = crate::measurement::Span::new("navigation.completion");
    let result = (|| {
        io.begin_guard()?;
        io.guard()?;
        let source = io.observe()?;
        io.guard()?;
        ensure!(
            source.before == source.after && source.before.index_matches,
            "Source identity/order not settled before navigation"
        );
        ensure!(
            source.image.dimensions() == (768, 1024),
            "Unknown navigation image dimensions"
        );
        let index = source
            .before
            .order
            .iter()
            .position(|p| p.id == source.before.owner.page)
            .context("Source absent from navigation order")?;
        let next = match direction {
            NavigationDirection::Next => index.checked_add(1),
            NavigationDirection::Previous => index.checked_sub(1),
        };
        let Some(target) = next
            .and_then(|n| source.before.order.get(n))
            .map(|p| p.id.clone())
        else {
            return Ok(NavigationCompletion::NoMovement);
        };
        // The gesture shares the physical input device. Observe read-only
        // intervals on either side without pretending to attribute own events.
        io.end_guard();
        io.swipe(direction)?;
        let deadline = io.now().saturating_add(Duration::from_secs(5));
        io.begin_guard()?;
        let source_image = DynamicImage::ImageLuma8(source.image.clone());
        let mut target_visit = None;
        let mut candidate: Option<GrayImage> = None;
        let mut source_confirmed = false;
        loop {
            io.guard()?;
            if io.now() >= deadline {
                if source_confirmed && target_visit.is_none() {
                    return Ok(NavigationCompletion::NoMovement);
                }
                anyhow::bail!("Destination not ready before deadline: pending identity, indistinguishable pixels or occupied chrome gutter/footer");
            }
            let current = io.observe()?;
            io.guard()?;
            ensure!(
                io.now() < deadline,
                "Navigation observation exceeded deadline"
            );
            check_metadata(&current.before, &source.before, &target, &mut target_visit)?;
            check_metadata(&current.after, &source.before, &target, &mut target_visit)?;
            ensure!(
                current.image.dimensions() == (768, 1024),
                "Navigation image dimensions changed"
            );
            let stable_owner = current.before == current.after && current.after.index_matches;
            let same_source = Workflow::is_same_page(
                &source_image,
                &DynamicImage::ImageLuma8(current.image.clone()),
            );
            source_confirmed =
                stable_owner && current.after.owner == source.before.owner && same_source;
            if stable_owner
                && current.after.owner.page == target
                && !same_source
                && chrome_ready(&current.image)
            {
                if candidate
                    .as_ref()
                    .is_some_and(|previous| same_pixels(previous, &current.image))
                {
                    io.guard()?;
                    ensure!(
                        io.now() < deadline,
                        "Navigation verification exceeded deadline"
                    );
                    return Ok(NavigationCompletion::Settled);
                }
                candidate = Some(current.image);
            } else {
                candidate = None;
            }
            io.pace(Duration::from_millis(50).min(deadline.saturating_sub(io.now())));
        }
    })();
    io.end_guard();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{device::native_page::OrderedPage, workflow::history::Owner};
    use image::Luma;
    use std::collections::VecDeque;

    fn metadata(target: bool) -> NavigationMetadata {
        NavigationMetadata {
            owner: Owner {
                document: "doc".into(),
                page: if target { "notes" } else { "source" }.into(),
                visit: if target { "1:2" } else { "1:1" }.into(),
                session: "5:6".into(),
            },
            order: [
                ("source", "a", Some(0)),
                ("notes", "ab", None),
                ("third", "b", Some(1)),
            ]
            .into_iter()
            .map(|(id, key, redirect)| OrderedPage {
                id: id.into(),
                key: key.into(),
                redirect,
            })
            .collect(),
            index_matches: true,
        }
    }
    fn frame(target: bool) -> Frame {
        let mut image = GrayImage::from_pixel(768, 1024, Luma([255]));
        if !target {
            for y in 300..500 {
                for x in 400..600 {
                    image.put_pixel(x, y, Luma([0]));
                }
            }
        }
        Frame {
            before: metadata(target),
            after: metadata(target),
            image,
        }
    }
    struct Scripted {
        frames: VecDeque<Frame>,
        last: Frame,
        clock: Duration,
        cost: Duration,
        swipes: usize,
        reads: usize,
        guard_calls: usize,
        cancel_at: Option<usize>,
        opens: usize,
        fail_open: Option<usize>,
        guarding: bool,
    }
    impl Scripted {
        fn new(frames: Vec<Frame>) -> Self {
            Self {
                last: frames.last().unwrap().clone(),
                frames: frames.into(),
                clock: Duration::ZERO,
                cost: Duration::from_millis(10),
                swipes: 0,
                reads: 0,
                guard_calls: 0,
                cancel_at: None,
                opens: 0,
                fail_open: None,
                guarding: false,
            }
        }
    }
    impl NavigationIo for Scripted {
        fn observe(&mut self) -> Result<Frame> {
            assert!(self.guarding);
            self.reads += 1;
            self.clock += self.cost;
            Ok(self.frames.pop_front().unwrap_or_else(|| self.last.clone()))
        }
        fn swipe(&mut self, _: NavigationDirection) -> Result<()> {
            assert!(!self.guarding);
            self.swipes += 1;
            self.clock += Duration::from_millis(200);
            Ok(())
        }
        fn now(&self) -> Duration {
            self.clock
        }
        fn pace(&mut self, duration: Duration) {
            assert!(duration <= Duration::from_millis(50));
            self.clock += duration;
        }
        fn begin_guard(&mut self) -> Result<()> {
            self.opens += 1;
            ensure!(self.fail_open != Some(self.opens), "Observer open failed");
            self.guarding = true;
            Ok(())
        }
        fn guard(&mut self) -> Result<()> {
            assert!(self.guarding);
            self.guard_calls += 1;
            ensure!(self.cancel_at != Some(self.guard_calls), "Input cancelled");
            Ok(())
        }
        fn end_guard(&mut self) {
            self.guarding = false;
        }
    }

    #[test]
    fn inked_source_to_blank_neighbor_needs_fresh_identity_and_pixels() {
        let mut metadata_ahead = frame(false);
        metadata_ahead.before = metadata(true);
        metadata_ahead.after = metadata(true);
        let mut pixels_ahead = frame(true);
        pixels_ahead.before = metadata(false);
        pixels_ahead.after = metadata(false);
        let mut lagging_index = frame(true);
        lagging_index.before.index_matches = false;
        lagging_index.after.index_matches = false;
        for pending in [
            vec![],
            vec![metadata_ahead.clone(), metadata_ahead],
            vec![pixels_ahead, lagging_index],
        ] {
            let mut frames = vec![frame(false)];
            frames.extend(pending.clone());
            frames.extend([frame(true), frame(true)]);
            let mut io = Scripted::new(frames);
            assert_eq!(
                navigate(&mut io, NavigationDirection::Next).unwrap(),
                NavigationCompletion::Settled
            );
            assert_eq!(io.swipes, 1);
            assert_eq!(io.reads, pending.len() + 3);
            assert!(!io.guarding);
        }
    }

    #[test]
    fn changing_render_and_actual_scrollbar_pair_wait_without_masking_pixels() {
        let mut partial = frame(true);
        for y in 400..600 {
            for x in 310..360 {
                partial.image.put_pixel(x, y, Luma([0]));
            }
        }
        let mut io = Scripted::new(vec![frame(false), partial, frame(true), frame(true)]);
        assert_eq!(
            navigate(&mut io, NavigationDirection::Next).unwrap(),
            NavigationCompletion::Settled
        );
        assert_eq!(io.reads, 4);
        let mut pending = frame(true);
        pending.image = image::load_from_memory(include_bytes!(
            "../../tests/fixtures/status-style/native-scrollbar-before.png"
        ))
        .unwrap()
        .to_luma8();
        let mut ready = frame(true);
        ready.image = image::load_from_memory(include_bytes!(
            "../../tests/fixtures/status-style/native-scrollbar-after.png"
        ))
        .unwrap()
        .to_luma8();
        let mut io = Scripted::new(vec![
            frame(false),
            pending.clone(),
            pending,
            ready.clone(),
            ready,
        ]);
        assert_eq!(
            navigate(&mut io, NavigationDirection::Next).unwrap(),
            NavigationCompletion::Settled
        );
        assert_eq!((io.swipes, io.reads), (1, 5));
    }

    #[test]
    fn identical_pages_occupied_gutter_and_late_capture_cannot_complete() {
        let mut same = frame(true);
        same.image = frame(false).image;
        let mut ink = frame(true);
        ink.image.put_pixel(737, 400, Luma([0]));
        for pending in [same, ink] {
            let mut io = Scripted::new(vec![frame(false), pending]);
            assert!(navigate(&mut io, NavigationDirection::Next).is_err());
            assert_eq!(io.swipes, 1);
            assert!(!io.guarding);
            assert!(io.clock < Duration::from_secs(6));
        }
        let blank_source = Frame {
            image: frame(true).image,
            ..frame(false)
        };
        let mut io = Scripted::new(vec![blank_source, frame(true)]);
        assert!(navigate(&mut io, NavigationDirection::Next).is_err());
        let mut io = Scripted::new(vec![frame(false), frame(true)]);
        io.cost = Duration::from_secs(5);
        assert!(navigate(&mut io, NavigationDirection::Next)
            .unwrap_err()
            .to_string()
            .contains("exceeded deadline"));
        assert_eq!((io.swipes, io.reads), (1, 2));
    }

    #[test]
    fn wrong_neighbor_owner_order_redirect_and_stale_visit_stop_after_one_swipe() {
        for fault in 0..6 {
            let mut wrong = frame(true);
            for state in [&mut wrong.before, &mut wrong.after] {
                match fault {
                    0 => state.owner.page = "third".into(),
                    1 => state.owner.document = "other".into(),
                    2 => state.owner.session = "restarted".into(),
                    3 => state.order.swap(1, 2),
                    4 => state.order[0].redirect = Some(1),
                    _ => state.owner.visit = "1:1".into(),
                }
            }
            let mut io = Scripted::new(vec![frame(false), wrong, frame(true), frame(true)]);
            assert!(navigate(&mut io, NavigationDirection::Next).is_err());
            assert_eq!((io.swipes, io.reads), (1, 2));
        }
        let mut io = Scripted::new(vec![frame(false), frame(true), frame(false), frame(true)]);
        assert!(navigate(&mut io, NavigationDirection::Next).is_err());
        assert_eq!(io.swipes, 1);
    }

    #[test]
    fn cancellation_open_failure_and_boundary_never_repeat_navigation() {
        for cancel in 1..=4 {
            let mut io = Scripted::new(vec![frame(false), frame(true), frame(true)]);
            io.cancel_at = Some(cancel);
            assert!(navigate(&mut io, NavigationDirection::Next).is_err());
            assert_eq!(io.swipes, usize::from(cancel > 2));
            assert!(!io.guarding);
        }
        for open in 1..=2 {
            let mut io = Scripted::new(vec![frame(false), frame(true)]);
            io.fail_open = Some(open);
            assert!(navigate(&mut io, NavigationDirection::Next).is_err());
            assert_eq!(io.swipes, open - 1);
            assert!(!io.guarding);
        }
        let mut io = Scripted::new(vec![frame(false)]);
        assert_eq!(
            navigate(&mut io, NavigationDirection::Previous).unwrap(),
            NavigationCompletion::NoMovement
        );
        assert_eq!(io.swipes, 0);
        let mut io = Scripted::new(vec![frame(false)]);
        assert_eq!(
            navigate(&mut io, NavigationDirection::Next).unwrap(),
            NavigationCompletion::NoMovement
        );
        assert_eq!(io.swipes, 1);
        assert!(!io.guarding);
    }
}
