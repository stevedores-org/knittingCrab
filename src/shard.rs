/// Aggregates results from sharded task execution.
/// Tracks success/failure for each shard and determines overall success.
#[derive(Debug, Clone)]
pub struct ShardAggregator {
    total: u32,
    results: Vec<Option<bool>>, // indexed by shard_index; None = not yet reported
}

impl ShardAggregator {
    /// Creates a new ShardAggregator for the given number of shards.
    pub fn new(total_shards: u32) -> Self {
        ShardAggregator {
            total: total_shards,
            results: vec![None; total_shards as usize],
        }
    }

    /// Records the result of a shard.
    /// Panics if index is out of bounds or if already recorded.
    pub fn record(&mut self, index: u32, success: bool) {
        let idx = index as usize;
        assert!(
            idx < self.results.len(),
            "shard index {} out of bounds for {} shards",
            index,
            self.total
        );
        assert!(
            self.results[idx].is_none(),
            "shard {} already reported",
            index
        );
        self.results[idx] = Some(success);
    }

    /// Returns true if all shards have reported a result.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.results.iter().all(|r| r.is_some())
    }

    /// Returns the overall success: true only if all shards succeeded.
    /// Returns false if any shard failed or if not all shards have reported.
    #[must_use]
    pub fn overall_success(&self) -> bool {
        self.results.iter().all(|r| matches!(r, Some(true)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_aggregator_correctness() {
        let mut agg = ShardAggregator::new(4);

        agg.record(0, true);
        agg.record(1, true);
        agg.record(2, false); // one shard fails
        agg.record(3, true);

        assert!(agg.is_complete());
        assert!(!agg.overall_success(), "should fail if any shard fails");
    }

    #[test]
    fn shard_aggregator_all_pass() {
        let mut agg = ShardAggregator::new(4);

        agg.record(0, true);
        agg.record(1, true);
        agg.record(2, true);
        agg.record(3, true);

        assert!(agg.is_complete());
        assert!(agg.overall_success(), "should pass if all shards pass");
    }

    #[test]
    fn shard_independence() {
        let mut agg = ShardAggregator::new(3);

        // Record out of order
        agg.record(2, true);
        agg.record(0, true);
        agg.record(1, false);

        assert!(agg.is_complete());
        assert!(!agg.overall_success());
    }

    #[test]
    fn incomplete_aggregation() {
        let mut agg = ShardAggregator::new(4);

        agg.record(0, true);
        agg.record(1, true);
        // Only 2 of 4 recorded

        assert!(!agg.is_complete());
        assert!(
            !agg.overall_success(),
            "incomplete should be false for success"
        );
    }
}
