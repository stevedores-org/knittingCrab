use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::{CoreError, Lease, LeaseState};
use knitting_crab_worker::{InMemoryLeaseStore, LeaseManager};
use std::time::Duration;

#[tokio::test]
async fn no_duplicate_leases() {
    let store = InMemoryLeaseStore::new();
    let manager = LeaseManager::new(store);

    let task_id = TaskId::new();
    let worker_id = WorkerId::new();
    let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

    manager.acquire(lease.clone()).await.unwrap();

    let result = manager.acquire(lease).await;
    assert!(matches!(result, Err(CoreError::AlreadyLeased)));
}

#[tokio::test]
async fn lease_expires_requeues() {
    let store = InMemoryLeaseStore::new();
    let manager = LeaseManager::new(store);

    let task_id = TaskId::new();
    let worker_id = WorkerId::new();
    let lease = Lease::new(task_id, worker_id, Duration::from_millis(1), 0);

    manager.acquire(lease).await.unwrap();

    std::thread::sleep(Duration::from_millis(10));

    let expired = manager.collect_expired().await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].state, LeaseState::Expired);
}

#[tokio::test]
async fn heartbeat_extends_lease() {
    let store = InMemoryLeaseStore::new();
    let manager = LeaseManager::new(store);

    let task_id = TaskId::new();
    let worker_id = WorkerId::new();
    let lease = Lease::new(task_id, worker_id, Duration::from_millis(1), 0);

    manager.acquire(lease).await.unwrap();

    std::thread::sleep(Duration::from_millis(5));

    manager
        .renew(task_id, Duration::from_secs(10))
        .await
        .unwrap();

    let expired = manager.collect_expired().await.unwrap();
    assert_eq!(expired.len(), 0);

    let lease = manager
        .store
        .get(task_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!lease.is_expired());
}
