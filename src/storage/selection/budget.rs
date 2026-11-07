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
    total_bytes: Option<u64>,
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
            total_bytes: None,
        }
    }
    pub(crate) fn combined_bytes(work_items: u64, bytes: u64) -> Self {
        let mut budget = Self::new(0, work_items, bytes, bytes);
        budget.total_bytes = Some(bytes);
        budget
    }
    pub fn usage(&self) -> SelectedReadUsage {
        self.usage
    }
    pub fn exhausted(&self) -> bool {
        self.exhausted
    }
    /// Irrevocably reserve a finite child allowance. Parent usage keeps the full
    /// reservation; callers cannot replenish it by dropping or replacing a child.
    pub(crate) fn reserve(
        &mut self,
        aggregates: u64,
        work_items: u64,
        metadata_bytes: u64,
        media_bytes: u64,
    ) -> Result<Self> {
        self.charge(aggregates, work_items, metadata_bytes, media_bytes)?;
        Ok(Self::new(
            aggregates,
            work_items,
            metadata_bytes,
            media_bytes,
        ))
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
            if self.total_bytes.is_some_and(|limit| {
                metadata_bytes
                    .checked_add(media_bytes)
                    .is_none_or(|total| total > limit)
            }) {
                self.exhausted = true;
                anyhow::bail!("combined archive work budget exhausted");
            }
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

#[cfg(test)]
mod reservation_tests {
    use super::*;
    #[test]
    fn dropped_child_does_not_replenish_the_parent_allowance() {
        let mut parent = SelectedReadBudget::new(1, 2, 3, 0);
        {
            let mut child = parent.reserve(1, 2, 3, 0).unwrap();
            child.charge(1, 2, 3, 0).unwrap();
        }
        assert!(parent.reserve(1, 0, 0, 0).is_err());
        assert!(parent.exhausted());
        assert!(parent.reserve(0, 0, 0, 0).is_err());
    }
}
