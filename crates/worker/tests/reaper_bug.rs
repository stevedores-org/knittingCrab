use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::lease::{Lease, LeaseState};
use knitting_crab_core::traits::LeaseStore;
use knitting_crab_worker::lease_manager::{InMemoryLeaseStore, LeaseManager};
use std::time::Duration;

#[tokio::test]
async fn test_active_leases_excludes_completed() {
    let store = InMemoryLeaseStore::new();
    let manager = LeaseManager::new(store.clone());

    let task_id = TaskId::new();
    let worker_id = WorkerId::new();
    let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

    // 1. Acquire lease
    manager.acquire(lease.clone()).await.unwrap();

    // 2. Complete lease
    manager.complete(task_id).await.unwrap();

    // 3. Check active leases
    // FIX: Should return 0 because it's completed.
    let active = store.active_leases().await.unwrap();
    assert_eq!(
        active.len(),
        0,
        "Should return 0 leases (completed lease filtered)"
    );
}

#[tokio::test]
async fn test_collect_expired_ignores_completed_tasks() {
    let store = InMemoryLeaseStore::new();
    let manager = LeaseManager::new(store.clone());

    let task_id = TaskId::new();
    let worker_id = WorkerId::new();
    // Short TTL so we can expire it easily
    let lease = Lease::new(task_id, worker_id, Duration::from_millis(1), 0);

    // 1. Acquire lease
    manager.acquire(lease.clone()).await.unwrap();

    // 2. Complete lease
    manager.complete(task_id).await.unwrap();

    // 3. Wait for expiration time to pass
    tokio::time::sleep(Duration::from_millis(10)).await;

    // 4. Collect expired
    // FIX: Should return 0 because it's completed, even if expired.
    let expired = manager.collect_expired().await.unwrap();

    assert_eq!(
        expired.len(),
        0,
        "Should collect 0 expired leases (completed lease ignored)"
    );

    // Check that state remains Completed
    let current = store.get(task_id).await.unwrap().unwrap();
    assert_eq!(current.state, LeaseState::Completed);
}
