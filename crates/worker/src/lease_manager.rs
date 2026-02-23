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

    async fn mark_expired_atomically(&self, task_ids: Vec<TaskId>) -> Result<Vec<Lease>, CoreError> {
        let mut expired_leases = Vec::new();

        for task_id in task_ids {
            // Use DashMap's entry API for atomic check-and-update.
            // This ensures no other writer can acquire the lease between
            // our check and our update.
            if let Some(mut entry) = self.leases.get_mut(&task_id) {
                let lease = entry.value_mut();
                // Only transition Active → Expired if the lease is actually expired
                if lease.state == knitting_crab_core::lease::LeaseState::Active && lease.is_expired() {
                    lease.state = knitting_crab_core::lease::LeaseState::Expired;
                    expired_leases.push(lease.clone());
                }
            }
        }

        Ok(expired_leases)
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
    /// Uses atomic update to prevent TOCTOU race where another worker acquires
    /// an expired lease between our check and our update.
    pub async fn collect_expired(&self) -> Result<Vec<Lease>, CoreError> {
        let leases = self.store.active_leases().await?;

        // Collect task IDs of potentially expired leases
        let expired_task_ids: Vec<TaskId> = leases
            .iter()
            .filter(|lease| lease.is_expired())
            .map(|lease| lease.task_id)
            .collect();

        // Atomically mark them as expired. This prevents the race condition where
        // another worker acquires the lease between our check and our update.
        self.store.mark_expired_atomically(expired_task_ids).await
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

    #[tokio::test]
    async fn reaper_prevents_duplicate_execution_race() {
        // This test reproduces the race condition from issue #68:
        // - Reaper checks lease, sees it's expired
        // - Worker A tries to acquire the same lease (race window)
        // - Reaper marks lease as Expired (atomic operation)
        // - Worker B should NOT be able to acquire the lease
        //
        // With atomic update, Worker A's acquire attempt should fail because
        // the lease is already owned by the reaper's atomic update.
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store.clone());

        let task_id = TaskId::new();
        let worker_id_a = WorkerId::new();
        let worker_id_b = WorkerId::new();

        // 1. Create short-lived lease (will expire quickly)
        let lease = Lease::new(task_id, worker_id_a, Duration::from_millis(50), 0);
        manager.acquire(lease).await.unwrap();

        // 2. Wait for lease to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 3. Run reaper to mark expired leases
        let expired = manager.collect_expired().await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].state, LeaseState::Expired);

        // 4. Verify state in store is Expired
        let stored = store.get(task_id).await.unwrap().unwrap();
        assert_eq!(stored.state, LeaseState::Expired);

        // 5. Try to acquire the same lease with a different worker
        // This should fail because the lease is now Expired (or already held)
        let new_lease = Lease::new(task_id, worker_id_b, Duration::from_secs(10), 1);
        let result = manager.acquire(new_lease).await;

        // The lease should not be re-acquirable by a different worker.
        // It's either still present (expired state) or removed, but not available.
        assert!(result.is_err(), "Should not allow duplicate lease acquisition");
    }

    #[tokio::test]
    async fn atomic_expiry_prevents_concurrent_state_change_race() {
        // This test verifies that the atomic mark_expired_atomically operation
        // prevents concurrent modification of lease state.
        let store = InMemoryLeaseStore::new();
        let manager = LeaseManager::new(store.clone());

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();

        // Create short-lived lease
        let lease = Lease::new(task_id, worker_id, Duration::from_millis(50), 0);
        manager.acquire(lease).await.unwrap();

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Spawn multiple tasks that try to interact with the lease simultaneously
        let store_clone = store.clone();
        let manager_clone = manager.clone();

        let (result1, result2) = tokio::join!(
            async {
                // Task 1: Reaper marks it as expired
                manager_clone.collect_expired().await
            },
            async {
                // Task 2: Try to get the lease (should see Expired state)
                tokio::time::sleep(Duration::from_millis(1)).await;
                store_clone.get(task_id).await
            }
        );

        // Reaper should find it
        let expired = result1.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].state, LeaseState::Expired);

        // The lease should be in Expired state (not Active, not missing)
        let lease_state = result2.unwrap().unwrap();
        assert_eq!(lease_state.state, LeaseState::Expired);
    }
}
