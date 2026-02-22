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
    pub async fn acquire(
        &self,
        lease: Lease,
    ) -> Result<(), CoreError> {
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
                let mut expired_lease = lease.clone();
                expired_lease.state = knitting_crab_core::LeaseState::Expired;
                self.store.update(expired_lease.clone()).await?;
                expired.push(expired_lease);
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

        let lease = manager
            .store
            .get(task_id)
            .await
            .unwrap()
            .unwrap();
        assert!(!lease.is_expired());
    }
}
