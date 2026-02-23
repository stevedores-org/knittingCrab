use crate::cache_index::CacheIndex;
use crate::node_registry::NodeRegistry;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::lease::Lease;
use knitting_crab_core::traits::{LeaseStore, Queue, TaskDescriptor};
use knitting_crab_scheduler::StubScheduler;
use knitting_crab_worker::InMemoryLeaseStore;
use std::sync::Arc;

#[derive(Clone)]
pub struct CoordinatorState {
    pub queue: Arc<StubScheduler>,
    pub lease_store: Arc<InMemoryLeaseStore>,
    pub node_registry: Arc<NodeRegistry>,
    pub cache_index: Arc<CacheIndex>,
}

impl Default for CoordinatorState {
    fn default() -> Self {
        Self::new()
    }
}

impl CoordinatorState {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(StubScheduler::new()),
            lease_store: Arc::new(InMemoryLeaseStore::new()),
            node_registry: Arc::new(NodeRegistry::new()),
            cache_index: Arc::new(CacheIndex::new()),
        }
    }

    pub fn enqueue_task(&self, task: TaskDescriptor) -> Result<(), CoreError> {
        self.queue.enqueue(task)
    }

    pub async fn dequeue_task(
        &self,
        worker_id: WorkerId,
    ) -> Result<Option<TaskDescriptor>, CoreError> {
        self.queue.dequeue(worker_id).await
    }

    pub async fn insert_lease(&self, lease: Lease) -> Result<(), CoreError> {
        self.lease_store.insert(lease).await
    }

    pub async fn get_lease(&self, task_id: TaskId) -> Result<Option<Lease>, CoreError> {
        self.lease_store.get(task_id).await
    }

    pub async fn update_lease(&self, lease: Lease) -> Result<(), CoreError> {
        self.lease_store.update(lease).await
    }

    pub async fn remove_lease(&self, task_id: TaskId) -> Result<(), CoreError> {
        self.lease_store.remove(task_id).await
    }

    pub async fn active_leases(&self) -> Result<Vec<Lease>, CoreError> {
        self.lease_store.active_leases().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[tokio::test]
    async fn state_creation() {
        let state = CoordinatorState::new();
        let leases = state.active_leases().await.unwrap();
        assert!(leases.is_empty());
    }

    #[tokio::test]
    async fn enqueue_and_dequeue_task() {
        let state = CoordinatorState::new();
        let task = TaskDescriptor {
            task_id: knitting_crab_core::ids::TaskId::new(),
            command: vec!["echo".to_string(), "hello".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: Default::default(),
            policy: Default::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
            goal: None,
            budget: None,
            test_gate: None,
        };
        let worker_id = WorkerId::new();

        state.enqueue_task(task.clone()).unwrap();
        let dequeued = state.dequeue_task(worker_id).await.unwrap();
        assert_eq!(dequeued.map(|t| t.task_id), Some(task.task_id));
    }
}
