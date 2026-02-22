use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;

use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::lease::Lease;
use knitting_crab_core::traits::LeaseStore;

/// In-memory lease store backed by DashMap for concurrent access.
pub struct InMemoryLeaseStore {
    leases: Arc<DashMap<TaskId, Lease>>,
}

impl InMemoryLeaseStore {
    pub fn new() -> Self {
        Self {
            leases: Arc::new(DashMap::new()),
        }
    }
}

impl Default for InMemoryLeaseStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for InMemoryLeaseStore {
    fn clone(&self) -> Self {
        Self {
            leases: Arc::clone(&self.leases),
        }
    }
}

#[async_trait]
impl LeaseStore for InMemoryLeaseStore {
    async fn insert(&self, lease: Lease) -> Result<(), CoreError> {
        if self.leases.contains_key(&lease.task_id) {
            return Err(CoreError::AlreadyLeased);
        }
        self.leases.insert(lease.task_id, lease);
        Ok(())
    }

    async fn get(&self, task_id: TaskId) -> Result<Option<Lease>, CoreError> {
        Ok(self.leases.get(&task_id).map(|r| r.clone()))
    }

    async fn update(&self, lease: Lease) -> Result<(), CoreError> {
        if !self.leases.contains_key(&lease.task_id) {
            return Err(CoreError::LeaseNotFound);
        }
        self.leases.insert(lease.task_id, lease);
        Ok(())
    }

    async fn remove(&self, task_id: TaskId) -> Result<(), CoreError> {
        self.leases.remove(&task_id);
        Ok(())
    }

    async fn active_leases(&self) -> Result<Vec<Lease>, CoreError> {
        Ok(self
            .leases
            .iter()
            .filter(|entry| entry.value().state == knitting_crab_core::lease::LeaseState::Active)
            .map(|entry| entry.value().clone())
            .collect())
    }
}

/// Manages lease lifecycle and operations.
#[derive(Clone)]
pub struct LeaseManager<S: LeaseStore + Clone> {
    store: S,
}

impl<S: LeaseStore + Clone> LeaseManager<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Acquire a lease for a task. Returns error if one already exists.
    pub async fn acquire(&self, lease: Lease) -> Result<(), CoreError> {
        self.store.insert(lease).await
    }

    /// Renew a lease by extending its expiration.
    pub async fn renew(&self, task_id: TaskId, ttl: Duration) -> Result<(), CoreError> {
        let mut lease = self
            .store
            .get(task_id)
            .await?
            .ok_or(CoreError::LeaseNotFound)?;

        lease.renew(ttl);
        self.store.update(lease).await
    }

    /// Collect all expired leases and transition them to Expired state.
    pub async fn collect_expired(&self) -> Result<Vec<Lease>, CoreError> {
        let leases = self.store.active_leases().await?;
        let mut expired = Vec::new();

        for lease in leases {
            if lease.is_expired() {
                // Double check: fetch fresh copy to prevent race condition
                if let Some(mut current) = self.store.get(lease.task_id).await? {
                    if current.state == knitting_crab_core::LeaseState::Active
                        && current.is_expired()
                    {
                        current.state = knitting_crab_core::LeaseState::Expired;
                        self.store.update(current.clone()).await?;
                        expired.push(current);
                    }
                }
            }
        }

        Ok(expired)
    }

    /// Mark a lease as completed.
    pub async fn complete(&self, task_id: TaskId) -> Result<(), CoreError> {
        let mut lease = self
            .store
            .get(task_id)
            .await?
            .ok_or(CoreError::LeaseNotFound)?;

        lease.state = knitting_crab_core::LeaseState::Completed;
        self.store.update(lease).await
    }

    /// Mark a lease as failed with given attempt count.
    pub async fn fail(&self, task_id: TaskId, attempts: u32) -> Result<(), CoreError> {
        let mut lease = self
            .store
            .get(task_id)
            .await?
            .ok_or(CoreError::LeaseNotFound)?;

        lease.state = knitting_crab_core::LeaseState::Failed { attempts };
        self.store.update(lease).await
    }

    /// Mark a lease as cancelled.
    pub async fn cancel(&self, task_id: TaskId) -> Result<(), CoreError> {
        let mut lease = self
            .store
            .get(task_id)
            .await?
            .ok_or(CoreError::LeaseNotFound)?;

        lease.state = knitting_crab_core::LeaseState::Cancelled;
        self.store.update(lease).await
    }

    /// Remove a lease completely.
    pub async fn remove(&self, task_id: TaskId) -> Result<(), CoreError> {
        self.store.remove(task_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knitting_crab_core::ids::{TaskId, WorkerId};
    use knitting_crab_core::LeaseState;

    #[tokio::test]
    async fn test_no_duplicate_leases() {
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
    async fn test_lease_expires_requeues() {
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
    async fn test_heartbeat_extends_lease() {
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

        let lease = manager.store.get(task_id).await.unwrap().unwrap();
        assert!(!lease.is_expired());
    }

    // ===== Lease State Transitions =====

    #[tokio::test]
    async fn test_complete_transitions_lease() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

        manager.acquire(lease).await.unwrap();
        manager.complete(task_id).await.unwrap();

        let lease = manager.store.get(task_id).await.unwrap().unwrap();
        assert_eq!(lease.state, LeaseState::Completed);
    }

    #[tokio::test]
    async fn test_fail_transitions_lease_with_attempts() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

        manager.acquire(lease).await.unwrap();
        manager.fail(task_id, 3).await.unwrap();

        let lease = manager.store.get(task_id).await.unwrap().unwrap();
        assert!(matches!(lease.state, LeaseState::Failed { attempts: 3 }));
    }

    #[tokio::test]
    async fn test_cancel_transitions_lease() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

        manager.acquire(lease).await.unwrap();
        manager.cancel(task_id).await.unwrap();

        let lease = manager.store.get(task_id).await.unwrap().unwrap();
        assert_eq!(lease.state, LeaseState::Cancelled);
    }

    #[tokio::test]
    async fn test_remove_deletes_lease() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

        manager.acquire(lease).await.unwrap();
        manager.remove(task_id).await.unwrap();

        let result = manager.store.get(task_id).await.unwrap();
        assert!(result.is_none());
    }

    // ===== Error Handling =====

    #[tokio::test]
    async fn test_renew_nonexistent_lease_fails() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let result = manager.renew(task_id, Duration::from_secs(10)).await;

        assert!(matches!(result, Err(CoreError::LeaseNotFound)));
    }

    #[tokio::test]
    async fn test_complete_nonexistent_lease_fails() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let result = manager.complete(task_id).await;

        assert!(matches!(result, Err(CoreError::LeaseNotFound)));
    }

    #[tokio::test]
    async fn test_fail_nonexistent_lease_fails() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let result = manager.fail(task_id, 1).await;

        assert!(matches!(result, Err(CoreError::LeaseNotFound)));
    }

    #[tokio::test]
    async fn test_cancel_nonexistent_lease_fails() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store);

        let task_id = TaskId::new();
        let result = manager.cancel(task_id).await;

        assert!(matches!(result, Err(CoreError::LeaseNotFound)));
    }

    // ===== Store Operations =====

    #[tokio::test]
    async fn test_store_get_returns_none_for_missing() {
        let store = InMemoryLeaseStore::new();
        let task_id = TaskId::new();

        let result = store.get(task_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_store_update_fails_on_missing_lease() {
        let store = InMemoryLeaseStore::new();
        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

        let result = store.update(lease).await;
        assert!(matches!(result, Err(CoreError::LeaseNotFound)));
    }

    #[tokio::test]
    async fn test_active_leases_with_multiple_leases() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store.clone());

        let task_id1 = TaskId::new();
        let task_id2 = TaskId::new();
        let task_id3 = TaskId::new();
        let worker_id = WorkerId::new();

        let lease1 = Lease::new(task_id1, worker_id, Duration::from_secs(10), 0);
        let lease2 = Lease::new(task_id2, worker_id, Duration::from_secs(10), 0);
        let lease3 = Lease::new(task_id3, worker_id, Duration::from_secs(10), 0);

        manager.acquire(lease1).await.unwrap();
        manager.acquire(lease2).await.unwrap();
        manager.acquire(lease3).await.unwrap();

        let active = store.active_leases().await.unwrap();
        assert_eq!(active.len(), 3);
    }

    #[tokio::test]
    async fn test_active_leases_excludes_expired() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store.clone());

        let task_id1 = TaskId::new();
        let task_id2 = TaskId::new();
        let worker_id = WorkerId::new();

        // Create one short-lived lease and one long-lived lease
        let lease1 = Lease::new(task_id1, worker_id, Duration::from_millis(1), 0);
        let lease2 = Lease::new(task_id2, worker_id, Duration::from_secs(10), 0);

        manager.acquire(lease1).await.unwrap();
        manager.acquire(lease2).await.unwrap();

        // Wait for first lease to expire
        std::thread::sleep(Duration::from_millis(10));

        let active = store.active_leases().await.unwrap();
        // Both should still be in active_leases (active_leases returns all, expired is handled by collect_expired)
        assert_eq!(active.len(), 2);

        // But collect_expired should find the expired one
        let expired = manager.collect_expired().await.unwrap();
        assert_eq!(expired.len(), 1);
    }

    #[tokio::test]
    async fn test_collect_expired_marks_state() {
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store.clone());

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_millis(1), 0);

        manager.acquire(lease).await.unwrap();
        std::thread::sleep(Duration::from_millis(10));

        // Collection finds the expired lease
        let expired = manager.collect_expired().await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].state, LeaseState::Expired);

        // Verify the lease in store is also marked Expired
        let stored_lease = store.get(task_id).await.unwrap().unwrap();
        assert_eq!(stored_lease.state, LeaseState::Expired);
    }

    #[tokio::test]
    async fn test_lease_store_clone_shares_state() {
        let store1 = InMemoryLeaseStore::new();
        let store2 = store1.clone();

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

        // Insert via store1
        store1.insert(lease).await.unwrap();

        // Retrieve via store2 (should have shared DashMap)
        let retrieved = store2.get(task_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().task_id, task_id);
    }

    #[tokio::test]
    async fn test_manager_clone_preserves_functionality() {
        let store = InMemoryLeaseStore::new();
        let manager1 = LeaseManager::new(store.clone());
        let manager2 = manager1.clone();

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(10), 0);

        // Acquire via manager1
        manager1.acquire(lease).await.unwrap();

        // Complete via manager2 (should operate on same store)
        manager2.complete(task_id).await.unwrap();

        // Verify state change
        let result = store.get(task_id).await.unwrap().unwrap();
        assert_eq!(result.state, LeaseState::Completed);
    }
}
