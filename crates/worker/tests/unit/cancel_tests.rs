use knitting_crab_worker::CancelToken;
use std::time::Duration;

#[tokio::test]
async fn cancel_queued_task() {
    let (token, guard) = CancelToken::new();
    let guard = guard.clone();
    assert!(!guard.is_cancelled());

    token.cancel();
    assert!(guard.is_cancelled());
}

#[tokio::test]
async fn cancel_running_task() {
    let (token, mut guard) = CancelToken::new();

    let handle = tokio::spawn(async move {
        guard.cancelled().await;
        true
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    token.cancel();

    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .unwrap()
        .unwrap();
    assert!(result);
}

#[tokio::test]
async fn cancel_idempotency() {
    let (token, guard) = CancelToken::new();

    token.cancel();
    token.cancel();
    token.cancel();

    assert!(guard.is_cancelled());
}
