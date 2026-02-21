use std::time::Duration;

use crate::types::WorkItem;

/// Configuration knobs for the scheduling algorithm.
#[derive(Debug, Clone)]
pub struct SchedulerPolicy {
    /// Maximum number of tasks that may run concurrently.
    pub max_concurrent_tasks: usize,
    /// Apply the aging bonus every N scheduler ticks.
    pub aging_interval_ticks: u32,
    /// Upper bound on the age bonus to avoid unbounded priority inversion.
    pub max_age_bonus: u32,
    /// Base of the exponential back-off in seconds.
    pub retry_backoff_base_secs: u64,
    /// Maximum automatic retries for a failing task.
    pub max_retries: u32,
}

impl Default for SchedulerPolicy {
    /// Returns a `SchedulerPolicy` with conservative, production-suitable
    /// defaults.
    fn default() -> Self {
        Self {
            max_concurrent_tasks: 4,
            aging_interval_ticks: 10,
            max_age_bonus: 50,
            retry_backoff_base_secs: 2,
            max_retries: 3,
        }
    }
}

impl SchedulerPolicy {
    /// Computes `base * 2^retry_count` as the back-off duration.
    ///
    /// The result is capped at ~2 hours to prevent absurdly long waits.
    pub fn compute_backoff(&self, retry_count: u32) -> Duration {
        let secs = self
            .retry_backoff_base_secs
            .saturating_mul(1_u64 << retry_count.min(63));
        Duration::from_secs(secs.min(7200))
    }

    /// Returns `true` for exit codes that warrant an automatic retry.
    ///
    /// - `0` → success (not retried)
    /// - `1` → task-level failure (not retried by default)
    /// - `2` → transient / retryable failure
    /// - `130` → SIGINT (consider retryable)
    pub fn is_retryable_exit_code(&self, exit_code: i32) -> bool {
        matches!(exit_code, 2 | 130)
    }

    /// Increments the `priority_age_bonus` of every task that has been waiting
    /// for at least `aging_interval_ticks` ticks, up to `max_age_bonus`.
    ///
    /// `tick` is the current scheduler tick counter.
    pub fn apply_aging(&self, tasks: &mut [WorkItem], tick: u32) {
        if !tick.is_multiple_of(self.aging_interval_ticks.max(1)) {
            return;
        }
        for task in tasks.iter_mut() {
            if task.priority_age_bonus < self.max_age_bonus {
                task.priority_age_bonus += 1;
            }
        }
    }

    /// Sorts `tasks` so that the highest-urgency task comes first.
    ///
    /// Ordering uses [`WorkItem::effective_priority_score`]: lower score ⟹
    /// higher urgency.
    pub fn sort_by_effective_priority(tasks: &mut [WorkItem]) {
        tasks.sort();
    }
}
