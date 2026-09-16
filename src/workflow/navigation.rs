use anyhow::{Context, Result};
use image::DynamicImage;
use log::{info, warn};

use super::Workflow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnOutcome {
    AlreadySource,
    Returned,
    Unconfirmed,
}

trait SourceNavigation {
    fn is_source(&mut self) -> Result<bool>;
    fn previous_page(&mut self) -> Result<()>;
}

fn return_once(navigation: &mut impl SourceNavigation) -> Result<ReturnOutcome> {
    if navigation
        .is_source()
        .context("Check source before return")?
    {
        info!("Already on original page; no return swipe needed");
        return Ok(ReturnOutcome::AlreadySource);
    }

    info!("Attempting one return swipe to original page");
    navigation.previous_page().context("Return swipe")?;
    if navigation.is_source().context("Verify return to source")? {
        info!("Confirmed back on original page");
        Ok(ReturnOutcome::Returned)
    } else {
        warn!("Return to original page unconfirmed; no additional swipe");
        Ok(ReturnOutcome::Unconfirmed)
    }
}

struct WorkflowNavigation<'a> {
    workflow: &'a mut Workflow,
    original: &'a DynamicImage,
}

impl SourceNavigation for WorkflowNavigation<'_> {
    fn is_source(&mut self) -> Result<bool> {
        self.workflow.verify_navigation_to(self.original)
    }

    fn previous_page(&mut self) -> Result<()> {
        self.workflow.navigate_to_previous_page()?;
        std::thread::sleep(std::time::Duration::from_millis(800));
        Ok(())
    }
}

pub(super) fn return_to_original_page(
    workflow: &mut Workflow,
    original: &DynamicImage,
) -> Result<ReturnOutcome> {
    return_once(&mut WorkflowNavigation { workflow, original })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct ScriptedNavigation {
        readings: VecDeque<Result<bool>>,
        swipe_fails: bool,
        actions: Vec<&'static str>,
    }

    impl ScriptedNavigation {
        fn new(readings: impl IntoIterator<Item = Result<bool>>) -> Self {
            Self {
                readings: readings.into_iter().collect(),
                swipe_fails: false,
                actions: Vec::new(),
            }
        }
    }

    impl SourceNavigation for ScriptedNavigation {
        fn is_source(&mut self) -> Result<bool> {
            self.actions.push("verify");
            self.readings.pop_front().expect("unexpected extra capture")
        }

        fn previous_page(&mut self) -> Result<()> {
            self.actions.push("previous");
            anyhow::ensure!(!self.swipe_fails, "input unavailable");
            Ok(())
        }
    }

    #[test]
    fn already_on_source_never_swipes_back() {
        let mut nav = ScriptedNavigation::new([Ok(true)]);
        assert_eq!(return_once(&mut nav).unwrap(), ReturnOutcome::AlreadySource);
        assert_eq!(nav.actions, ["verify"]);
    }

    #[test]
    fn successful_return_has_one_swipe_between_two_checks() {
        let mut nav = ScriptedNavigation::new([Ok(false), Ok(true)]);
        assert_eq!(return_once(&mut nav).unwrap(), ReturnOutcome::Returned);
        assert_eq!(nav.actions, ["verify", "previous", "verify"]);
    }

    #[test]
    fn failed_return_stops_instead_of_swiping_multiple_pages() {
        let mut nav = ScriptedNavigation::new([Ok(false), Ok(false)]);
        assert_eq!(return_once(&mut nav).unwrap(), ReturnOutcome::Unconfirmed);
        assert_eq!(nav.actions, ["verify", "previous", "verify"]);
    }

    #[test]
    fn initial_capture_error_never_moves_the_page() {
        let mut nav = ScriptedNavigation::new([Err(anyhow::anyhow!("capture unavailable"))]);
        assert!(return_once(&mut nav).is_err());
        assert_eq!(nav.actions, ["verify"]);
    }

    #[test]
    fn post_swipe_capture_error_never_retries_navigation() {
        let mut nav =
            ScriptedNavigation::new([Ok(false), Err(anyhow::anyhow!("capture unavailable"))]);
        assert!(return_once(&mut nav).is_err());
        assert_eq!(nav.actions, ["verify", "previous", "verify"]);
    }

    #[test]
    fn failed_input_stops_without_another_swipe_or_capture() {
        let mut nav = ScriptedNavigation::new([Ok(false)]);
        nav.swipe_fails = true;
        assert!(return_once(&mut nav).is_err());
        assert_eq!(nav.actions, ["verify", "previous"]);
    }
}
