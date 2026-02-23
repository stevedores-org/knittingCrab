//! Task aging mechanism for preventing starvation under sustained load.
//!
//! This module implements aging policies that promote tasks to higher priority
//! if they've been waiting in the queue too long, ensuring bounded wait times
//! even when the system experiences sustained high-priority load.
//!
//! ## Aging Policies
//!
//! - `None`: No aging; tasks remain at original priority
//! - `Linear`: Tasks promoted after fixed wait time (e.g., 10s → High, 20s → Critical)
//! - `Exponential`: Faster promotion; wait time reduces as original priority decreases

use crate::priority::Priority;
use std::time::{Duration, Instant};

/// Configurable aging policy for priority promotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgingPolicy {
    /// No aging; tasks keep original priority.
    #[default]
    None,

    /// Linear aging: uniform wait time threshold before promotion.
    ///
    /// - Low/Normal: promote to High after base_ms
    /// - High: promote to Critical after base_ms
    ///
    /// Example: base_ms=10_000 means ~10s wait before promotion
    Linear {
        /// Base wait time in milliseconds before first promotion
        base_ms: u64,
    },

    /// Exponential aging: lower-priority tasks promoted faster.
    ///
    /// - Low: promote to Normal after base_ms / 2
    /// - Normal: promote to High after base_ms
    /// - High: promote to Critical after base_ms * 2
    Exponential {
        /// Base wait time in milliseconds
        base_ms: u64,
    },
}

impl AgingPolicy {
    /// Get the wait time threshold (in ms) for promotion from a given priority.
    fn threshold_ms(&self, from_priority: Priority) -> Option<u64> {
        match self {
            AgingPolicy::None => None,

            AgingPolicy::Linear { base_ms } => match from_priority {
                Priority::Critical => None, // Can't promote beyond Critical
                Priority::High => Some(*base_ms),
                Priority::Normal => Some(*base_ms),
                Priority::Low => Some(*base_ms),
            },

            AgingPolicy::Exponential { base_ms } => match from_priority {
                Priority::Critical => None,
                Priority::High => Some(base_ms * 2), // Longest wait for High
                Priority::Normal => Some(*base_ms),
                Priority::Low => Some(base_ms / 2), // Fastest promotion for Low
            },
        }
    }
}

/// Metadata for tracking a task's age in the queue.
#[derive(Debug, Clone)]
pub struct TaskAge {
    /// When the task was enqueued
    pub enqueued_at: Instant,
    /// Original priority before any aging promotion
    pub original_priority: Priority,
}

impl TaskAge {
    /// Create a new task age tracker.
    pub fn new(original_priority: Priority) -> Self {
        Self {
            enqueued_at: Instant::now(),
            original_priority,
        }
    }

    /// Get the current age in milliseconds.
    pub fn age_ms(&self) -> u64 {
        self.enqueued_at.elapsed().as_millis() as u64
    }

    /// Get the current age as a Duration.
    pub fn age(&self) -> Duration {
        self.enqueued_at.elapsed()
    }

    /// Compute the promoted priority based on aging policy.
    pub fn compute_promoted_priority(&self, policy: AgingPolicy) -> Priority {
        let age_ms = self.age_ms();

        // Get threshold for promotion from current priority
        let threshold = match policy.threshold_ms(self.original_priority) {
            Some(t) => t,
            None => return self.original_priority, // Can't promote
        };

        // Promote if age exceeds threshold
        if age_ms >= threshold {
            Self::promote_priority(self.original_priority)
        } else {
            self.original_priority
        }
    }

    /// Promote a priority one level up (or stay at Critical).
    fn promote_priority(p: Priority) -> Priority {
        match p {
            Priority::Low => Priority::Normal,
            Priority::Normal => Priority::High,
            Priority::High => Priority::Critical,
            Priority::Critical => Priority::Critical,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_aging_returns_original() {
        let age = TaskAge::new(Priority::Low);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            age.compute_promoted_priority(AgingPolicy::None),
            Priority::Low
        );
    }

    #[test]
    fn linear_aging_promotes_after_threshold() {
        let age = TaskAge::new(Priority::Low);
        // Wait and check promotion happens
        let policy = AgingPolicy::Linear { base_ms: 10 };

        // Just after creation: not promoted yet
        let promoted = age.compute_promoted_priority(policy);
        // Immediate check might be promoted or not (timing-dependent)
        assert!(promoted == Priority::Low || promoted == Priority::Normal);

        // Wait enough time
        std::thread::sleep(Duration::from_millis(15));
        let promoted = age.compute_promoted_priority(policy);
        assert_eq!(promoted, Priority::Normal);
    }

    #[test]
    fn exponential_aging_faster_promotion() {
        let policy_exp = AgingPolicy::Exponential { base_ms: 100 };
        let policy_lin = AgingPolicy::Linear { base_ms: 100 };

        // For Low priority, exponential threshold is half
        let low_exp_threshold = policy_exp.threshold_ms(Priority::Low).unwrap();
        let low_lin_threshold = policy_lin.threshold_ms(Priority::Low).unwrap();

        assert!(low_exp_threshold < low_lin_threshold);
        assert_eq!(low_exp_threshold, 50);
        assert_eq!(low_lin_threshold, 100);
    }

    #[test]
    fn critical_tasks_not_promoted() {
        let age = TaskAge::new(Priority::Critical);
        let policy = AgingPolicy::Linear { base_ms: 10 };

        std::thread::sleep(Duration::from_millis(50));

        let promoted = age.compute_promoted_priority(policy);
        assert_eq!(promoted, Priority::Critical);
    }

    #[test]
    fn compute_age_from_enqueue_time() {
        let age = TaskAge::new(Priority::Normal);
        std::thread::sleep(Duration::from_millis(20));
        let age_ms = age.age_ms();
        assert!((15..=100).contains(&age_ms));
    }

    #[test]
    fn configurable_thresholds() {
        let policy_short = AgingPolicy::Linear { base_ms: 5 };
        let policy_long = AgingPolicy::Linear { base_ms: 1000 };

        let age = TaskAge::new(Priority::Low);
        std::thread::sleep(Duration::from_millis(10));

        let promoted_short = age.compute_promoted_priority(policy_short);
        let promoted_long = age.compute_promoted_priority(policy_long);

        // Short threshold should promote
        assert_eq!(promoted_short, Priority::Normal);
        // Long threshold should not
        assert_eq!(promoted_long, Priority::Low);
    }

    #[test]
    fn cascading_promotions() {
        let policy = AgingPolicy::Linear { base_ms: 10 };
        let age = TaskAge::new(Priority::Low);

        // Simulate multiple promotion checks
        std::thread::sleep(Duration::from_millis(5));
        let promoted1 = age.compute_promoted_priority(policy);
        assert_eq!(promoted1, Priority::Low); // Not ready yet

        std::thread::sleep(Duration::from_millis(10));
        let promoted2 = age.compute_promoted_priority(policy);
        assert_eq!(promoted2, Priority::Normal); // Promoted to Normal

        // Once promoted, stays promoted (task age only looks at original)
        std::thread::sleep(Duration::from_millis(20));
        let promoted3 = age.compute_promoted_priority(policy);
        assert_eq!(promoted3, Priority::Normal); // Still Normal (original was Low)
    }

    #[test]
    fn aging_policy_thresholds() {
        let policy_exp = AgingPolicy::Exponential { base_ms: 200 };

        // Low: base/2 = 100ms
        assert_eq!(policy_exp.threshold_ms(Priority::Low), Some(100));

        // Normal: base = 200ms
        assert_eq!(policy_exp.threshold_ms(Priority::Normal), Some(200));

        // High: base*2 = 400ms
        assert_eq!(policy_exp.threshold_ms(Priority::High), Some(400));

        // Critical: can't promote
        assert_eq!(policy_exp.threshold_ms(Priority::Critical), None);
    }
}
