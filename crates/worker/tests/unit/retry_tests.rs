use knitting_crab_core::retry::{ExitOutcome, RetryDecision, RetryPolicy};
use knitting_crab_worker::RetryHandler;
use std::time::Duration;

#[test]
fn retries_with_exponential_backoff() {
    let policy = RetryPolicy::default();

    let decision_0 = RetryHandler::decide(ExitOutcome::FailedWithCode(1), &policy, 0);
    let decision_1 = RetryHandler::decide(ExitOutcome::FailedWithCode(1), &policy, 1);

    match (decision_0, decision_1) {
        (RetryDecision::Retry { delay: d0 }, RetryDecision::Retry { delay: d1 }) => {
            assert_eq!(d0, Duration::from_millis(100));
            assert_eq!(d1, Duration::from_millis(200));
        }
        _ => panic!("expected retry decisions"),
    }
}

#[test]
fn permanent_fail_after_max_attempts() {
    let policy = RetryPolicy {
        max_attempts: 3,
        ..Default::default()
    };

    let decision = RetryHandler::decide(ExitOutcome::FailedWithCode(1), &policy, 2);
    assert!(matches!(decision, RetryDecision::Abandon));
}

#[test]
fn retry_only_on_retryable_exit_codes() {
    let policy = RetryPolicy {
        retryable_exit_codes: vec![1, 2],
        ..Default::default()
    };

    let retryable = RetryHandler::decide(ExitOutcome::FailedWithCode(1), &policy, 0);
    let non_retryable = RetryHandler::decide(ExitOutcome::FailedWithCode(127), &policy, 0);

    assert!(matches!(retryable, RetryDecision::Retry { .. }));
    assert!(matches!(non_retryable, RetryDecision::Complete));
}
