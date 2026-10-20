//! One receiving request's cumulative content-I/O allowance; no domain policy.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectedReadUsage {
    pub aggregates: u64,
    pub work_items: u64,
    pub metadata_bytes: u64,
    pub media_bytes: u64,
}

/// Share one budget across catalogue and aggregate reads. Exhaustion is sticky;
/// the caller must discard an incomplete multi-aggregate receiving result.
pub struct SelectedReadBudget {
    limits: SelectedReadUsage,
    usage: SelectedReadUsage,
    exhausted: bool,
}
impl SelectedReadBudget {
    pub fn new(aggregates: u64, work_items: u64, metadata_bytes: u64, media_bytes: u64) -> Self {
        Self {
            limits: SelectedReadUsage {
                aggregates,
                work_items,
                metadata_bytes,
                media_bytes,
            },
            usage: SelectedReadUsage::default(),
            exhausted: false,
        }
    }
    pub fn usage(&self) -> SelectedReadUsage {
        self.usage
    }
    pub fn exhausted(&self) -> bool {
        self.exhausted
    }
    pub(crate) fn charge(
        &mut self,
        aggregates: u64,
        work_items: u64,
        metadata_bytes: u64,
        media_bytes: u64,
    ) -> Result<()> {
        ensure!(!self.exhausted, "selected read budget exhausted");
        let add = |used: u64, delta: u64, limit: u64| {
            used.checked_add(delta).filter(|next| *next <= limit)
        };
        let next = (
            add(self.usage.aggregates, aggregates, self.limits.aggregates),
            add(self.usage.work_items, work_items, self.limits.work_items),
            add(
                self.usage.metadata_bytes,
                metadata_bytes,
                self.limits.metadata_bytes,
            ),
            add(self.usage.media_bytes, media_bytes, self.limits.media_bytes),
        );
        if let (Some(aggregates), Some(work_items), Some(metadata_bytes), Some(media_bytes)) = next
        {
            self.usage = SelectedReadUsage {
                aggregates,
                work_items,
                metadata_bytes,
                media_bytes,
            };
            Ok(())
        } else {
            self.exhausted = true;
            anyhow::bail!("selected read budget exhausted")
        }
    }
}
