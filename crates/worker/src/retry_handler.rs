use knitting_crab_core::retry::{ExitOutcome, RetryDecision, RetryPolicy};
use std::time::Duration;

/// Pure, stateless retry decision logic.
pub struct RetryHandler;

impl RetryHandler {
    /// Decide whether to retry based on outcome and policy.
    pub fn decide(
        outcome: ExitOutcome,
        policy: &RetryPolicy,
        attempt: u32,
    ) -> RetryDecision {
        if !outcome.is_retryable(policy) {
            return RetryDecision::Complete;
        }

        if attempt + 1 >= policy.max_attempts {
            return RetryDecision::Abandon;
        }

        let delay = policy.backoff_for(attempt);
        RetryDecision::Retry { delay }
    }

    /// Generate backoff ladder for testing.
    pub fn backoff_ladder(policy: &RetryPolicy) -> Vec<Duration> {
        (0..policy.max_attempts)
            .map(|attempt| policy.backoff_for(attempt))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_completes() {
        let policy = RetryPolicy::default();
        let decision = RetryHandler::decide(ExitOutcome::Success, &policy, 0);
        assert!(matches!(decision, RetryDecision::Complete));
    }

    #[test]
    fn test_retryable_code_retries() {
        let policy = RetryPolicy::default();
        let decision = RetryHandler::decide(ExitOutcome::FailedWithCode(1), &policy, 0);
        assert!(matches!(decision, RetryDecision::Retry { .. }));
    }

    #[test]
    fn test_max_attempts_abandons() {
        let policy = RetryPolicy {
            max_attempts: 3,
            ..Default::default()
        };
        let decision = RetryHandler::decide(ExitOutcome::FailedWithCode(1), &policy, 2);
        assert!(matches!(decision, RetryDecision::Abandon));
    }

    #[test]
    fn test_non_retryable_code_completes() {
        let policy = RetryPolicy {
            retryable_exit_codes: vec![1, 2],
            ..Default::default()
        };
        let decision = RetryHandler::decide(ExitOutcome::FailedWithCode(127), &policy, 0);
        assert!(matches!(decision, RetryDecision::Complete));
    }

    #[test]
    fn test_timeout_retries() {
        let policy = RetryPolicy::default();
        let decision = RetryHandler::decide(ExitOutcome::TimedOut, &policy, 0);
        assert!(matches!(decision, RetryDecision::Retry { .. }));
    }

    #[test]
    fn test_backoff_ladder() {
        let policy = RetryPolicy::default();
        let ladder = RetryHandler::backoff_ladder(&policy);
        assert_eq!(ladder.len(), 3);
        assert_eq!(ladder[0], Duration::from_millis(100));
        assert_eq!(ladder[1], Duration::from_millis(200));
        assert_eq!(ladder[2], Duration::from_millis(400));
    }
}
