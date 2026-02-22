use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Retry policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of attempts (default: 3).
    pub max_attempts: u32,

    /// Initial backoff duration (default: 100ms).
    pub initial_backoff: Duration,

    /// Backoff multiplier for exponential growth (default: 2.0).
    pub backoff_multiplier: f64,

    /// Maximum backoff duration (default: 30s).
    pub max_backoff: Duration,

    /// Exit codes that should trigger retry. Empty means all codes are retryable.
    pub retryable_exit_codes: Vec<i32>,
}

impl RetryPolicy {
    /// Compute backoff duration for a given attempt (0-indexed).
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        let backoff = self.initial_backoff.as_secs_f64()
            * self.backoff_multiplier.powi(attempt as i32);
        let backoff_secs = backoff.min(self.max_backoff.as_secs_f64());
        Duration::from_secs_f64(backoff_secs)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(100),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(30),
            retryable_exit_codes: vec![],
        }
    }
}

/// The outcome of a process execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExitOutcome {
    /// Process exited successfully with code 0.
    Success,

    /// Process exited with a non-zero code.
    FailedWithCode(i32),

    /// Process was killed by a signal.
    KilledBySignal(i32),

    /// Process exceeded timeout.
    TimedOut,
}

impl ExitOutcome {
    /// Check if this outcome is retryable according to the policy.
    pub fn is_retryable(&self, policy: &RetryPolicy) -> bool {
        match self {
            ExitOutcome::Success => false,
            ExitOutcome::FailedWithCode(code) => {
                if policy.retryable_exit_codes.is_empty() {
                    true
                } else {
                    policy.retryable_exit_codes.contains(code)
                }
            }
            ExitOutcome::KilledBySignal(_) => true,
            ExitOutcome::TimedOut => true,
        }
    }
}

/// Decision on whether to retry a task.
#[derive(Debug, Clone, Copy)]
pub enum RetryDecision {
    /// Retry after this delay.
    Retry { delay: Duration },

    /// Do not retry; give up permanently.
    Abandon,

    /// Task completed successfully.
    Complete,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_retry_policy() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.initial_backoff, Duration::from_millis(100));
        assert_eq!(policy.backoff_multiplier, 2.0);
        assert_eq!(policy.max_backoff, Duration::from_secs(30));
        assert!(policy.retryable_exit_codes.is_empty());
    }

    #[test]
    fn test_backoff_exponential() {
        let policy = RetryPolicy::default();
        let backoff_0 = policy.backoff_for(0);
        let backoff_1 = policy.backoff_for(1);
        let backoff_2 = policy.backoff_for(2);

        assert_eq!(backoff_0, Duration::from_millis(100));
        assert_eq!(backoff_1, Duration::from_millis(200));
        assert_eq!(backoff_2, Duration::from_millis(400));
    }

    #[test]
    fn test_backoff_capped_at_max() {
        let policy = RetryPolicy {
            max_attempts: 10,
            initial_backoff: Duration::from_secs(1),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(5),
            retryable_exit_codes: vec![],
        };

        let backoff_10 = policy.backoff_for(10);
        assert_eq!(backoff_10, Duration::from_secs(5));
    }

    #[test]
    fn test_exit_outcome_success_not_retryable() {
        let policy = RetryPolicy::default();
        assert!(!ExitOutcome::Success.is_retryable(&policy));
    }

    #[test]
    fn test_exit_outcome_signal_is_retryable() {
        let policy = RetryPolicy::default();
        assert!(ExitOutcome::KilledBySignal(15).is_retryable(&policy));
    }

    #[test]
    fn test_exit_outcome_timeout_is_retryable() {
        let policy = RetryPolicy::default();
        assert!(ExitOutcome::TimedOut.is_retryable(&policy));
    }

    #[test]
    fn test_exit_outcome_code_all_retryable() {
        let policy = RetryPolicy::default();
        assert!(ExitOutcome::FailedWithCode(1).is_retryable(&policy));
        assert!(ExitOutcome::FailedWithCode(127).is_retryable(&policy));
    }

    #[test]
    fn test_exit_outcome_code_selective_retryable() {
        let policy = RetryPolicy {
            retryable_exit_codes: vec![1, 2],
            ..Default::default()
        };
        assert!(ExitOutcome::FailedWithCode(1).is_retryable(&policy));
        assert!(!ExitOutcome::FailedWithCode(127).is_retryable(&policy));
    }
}
