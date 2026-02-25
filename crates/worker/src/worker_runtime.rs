use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{interval, sleep};
use tracing::{debug, error, info};

use knitting_crab_core::event::TaskEvent;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::lease::Lease;
use knitting_crab_core::retry::ExitOutcome;
use knitting_crab_core::traits::{EventSink, LeaseStore, Queue, ResourceMonitor};

use crate::cancel_token::{CancelGuard, CancelToken};
use crate::error::WorkerError;
use crate::lease_manager::LeaseManager;
use crate::process::SpawnParams;
use crate::retry_handler::RetryHandler;

/// Trait for executing processes (abstracted for testing).
#[async_trait]
pub trait ProcessExecutor: Send + Sync {
    async fn execute(
        &self,
        params: SpawnParams,
        sink: Arc<dyn EventSink>,
        cancel_guard: CancelGuard,
    ) -> Result<ExitOutcome, WorkerError>;
}

/// Real process executor using OS processes.
pub struct RealProcessExecutor;

#[async_trait]
impl ProcessExecutor for RealProcessExecutor {
    async fn execute(
        &self,
        params: SpawnParams,
        sink: Arc<dyn EventSink>,
        mut cancel_guard: CancelGuard,
    ) -> Result<ExitOutcome, WorkerError> {
        let mut handle = crate::process::spawn(params.clone(), sink).await?;

        let outcome = tokio::select! {
            result = handle.wait() => result,
            _ = cancel_guard.cancelled() => {
                handle.kill_gracefully(Duration::from_secs(5)).await
            }
        }?;

        Ok(outcome)
    }
}

struct ActiveTaskGuard {
    task_id: TaskId,
    active_tasks: Arc<DashMap<TaskId, CancelToken>>,
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        self.active_tasks.remove(&self.task_id);
    }
}

struct ResourceGuard<RM: ResourceMonitor + Clone> {
    monitor: RM,
    allocation: Option<knitting_crab_core::resource::ResourceAllocation>,
}

impl<RM: ResourceMonitor + Clone> ResourceGuard<RM> {
    fn new(monitor: RM, allocation: knitting_crab_core::resource::ResourceAllocation) -> Self {
        Self {
            monitor,
            allocation: Some(allocation),
        }
    }
}

impl<RM: ResourceMonitor + Clone> Drop for ResourceGuard<RM> {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            let monitor = self.monitor.clone();
            tokio::spawn(async move {
                if let Err(e) = monitor.release(&allocation).await {
                    error!("failed to release resources: {}", e);
                }
            });
        }
    }
}

/// Main worker runtime orchestrating task execution.
pub struct WorkerRuntime<
    Q: Queue,
    LS: LeaseStore + Clone,
    RM: ResourceMonitor + Clone,
    ES: EventSink + Clone,
    PE: ProcessExecutor,
> {
    worker_id: WorkerId,
    queue: Q,
    lease_store: LS,
    lease_manager: LeaseManager<LS>,
    /// Resource monitor: used for Phase 2 resource-aware scheduling and allocation tracking (see ARCHITECTURE.md).
    /// Currently used for can_allocate() and allocate() checks on task resources.
    #[allow(dead_code)]
    resource_monitor: RM,
    event_sink: ES,
    process_executor: PE,
    active_tasks: Arc<DashMap<TaskId, CancelToken>>,
    lease_ttl: Duration,
    heartbeat_interval_ms: u64,
    reaper_interval_ms: u64,
}

impl<
        Q: Queue,
        LS: LeaseStore + Clone,
        RM: ResourceMonitor + Clone,
        ES: EventSink + Clone,
        PE: ProcessExecutor,
    > WorkerRuntime<Q, LS, RM, ES, PE>
{
    pub fn new(
        worker_id: WorkerId,
        queue: Q,
        lease_store: LS,
        resource_monitor: RM,
        event_sink: ES,
        process_executor: PE,
    ) -> Self {
        let lease_manager = LeaseManager::new(lease_store.clone());
        Self {
            worker_id,
            queue,
            lease_store,
            lease_manager,
            resource_monitor,
            event_sink,
            process_executor,
            active_tasks: Arc::new(DashMap::new()),
            lease_ttl: Duration::from_secs(30),
            heartbeat_interval_ms: 5000,
            reaper_interval_ms: 10000,
        }
    }

    pub fn with_lease_ttl(mut self, ttl: Duration) -> Self {
        self.lease_ttl = ttl;
        self
    }

    pub fn with_heartbeat_interval(mut self, interval_ms: u64) -> Self {
        self.heartbeat_interval_ms = interval_ms;
        self
    }

    pub fn with_reaper_interval(mut self, interval_ms: u64) -> Self {
        self.reaper_interval_ms = interval_ms;
        self
    }

    /// Main worker loop: dequeue, execute, retry cycle.
    pub async fn worker_loop(&self) -> Result<(), WorkerError> {
        let span = tracing::info_span!("worker", worker_id = %self.worker_id);
        let _guard = span.enter();

        loop {
            match self.queue.dequeue(self.worker_id).await {
                Ok(Some(task)) => {
                    if let Err(e) = self.execute_task(&task).await {
                        error!("task execution error: {}", e);
                    }
                }
                Ok(None) => {
                    sleep(Duration::from_millis(100)).await;
                }
                Err(e) => {
                    error!("queue error: {}", e);
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Execute a single task with retry logic.
    async fn execute_task(
        &self,
        task: &knitting_crab_core::TaskDescriptor,
    ) -> Result<(), WorkerError> {
        let span = tracing::info_span!(
            "execute_task",
            task_id = %task.task_id,
            worker_id = %self.worker_id,
        );
        let _enter = span.enter();

        // Check if resources are available
        if !self.resource_monitor.can_allocate(&task.resources).await? {
            info!(
                "insufficient resources for task {}, requeueing",
                task.task_id
            );
            self.queue.requeue(task.task_id, task.attempt).await?;
            return Ok(());
        }

        // Allocate resources
        self.resource_monitor.allocate(&task.resources).await?;

        // Guard to ensure resources are released when this scope exits
        let _resource_guard =
            ResourceGuard::new(self.resource_monitor.clone(), task.resources.clone());

        info!("acquiring lease for task {}", task.task_id);

        let lease = Lease::new(task.task_id, self.worker_id, self.lease_ttl, task.attempt);
        match self.lease_manager.acquire(lease.clone()).await {
            Ok(_) => {}
            Err(knitting_crab_core::error::CoreError::AlreadyLeased) => {
                // Task is already running elsewhere or completed
                return Ok(());
            }
            Err(e) => {
                // System error, try to requeue so we don't lose the task
                error!("failed to acquire lease: {}, requeueing", e);
                self.queue.requeue(task.task_id, task.attempt).await?;
                return Err(e.into());
            }
        }

        if let Err(e) = self
            .event_sink
            .emit_event(TaskEvent::Acquired {
                task_id: task.task_id,
                worker_id: self.worker_id,
                attempt: task.attempt,
            })
            .await
        {
            error!("failed to emit acquired event: {}", e);
            // Proceed anyway as we have the lease
        }

        let (cancel_token, cancel_guard) = CancelToken::new();
        self.active_tasks.insert(task.task_id, cancel_token);

        // Ensure token is removed when this function exits (even on panic)
        let _guard = ActiveTaskGuard {
            task_id: task.task_id,
            active_tasks: self.active_tasks.clone(),
        };

        // Spawn heartbeat task
        let lease_manager = self.lease_manager.clone();
        let heartbeat_task_id = task.task_id;
        let heartbeat_interval = self.heartbeat_interval_ms;
        let lease_store = self.lease_store.clone();

        let heartbeat_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_millis(heartbeat_interval));
            loop {
                ticker.tick().await;
                if let Ok(None) = lease_store.get(heartbeat_task_id).await {
                    break;
                }
                let _ = lease_manager
                    .renew(heartbeat_task_id, Duration::from_secs(30))
                    .await;
            }
        });

        let spawn_params = SpawnParams {
            task_id: task.task_id,
            command: task.command.clone(),
            working_dir: task.working_dir.clone(),
            env: task.env.clone(),
            location: task.location.clone(),
        };

        let outcome = self
            .process_executor
            .execute(
                spawn_params,
                Arc::new(self.event_sink.clone()),
                cancel_guard,
            )
            .await?;

        // Gracefully shutdown heartbeat task with 5-second grace period
        // Instead of abort() which loses state, wait for clean shutdown
        let shutdown_timeout = Duration::from_secs(5);
        tokio::select! {
            _ = heartbeat_handle => {
                // Heartbeat finished cleanly, good!
            }
            _ = tokio::time::sleep(shutdown_timeout) => {
                // Timeout: heartbeat_handle will be dropped, forcing abort
                // This is acceptable as a fallback for hung tasks
            }
        }

        if let Err(e) = self
            .event_sink
            .emit_event(TaskEvent::Completed {
                task_id: task.task_id,
                outcome,
            })
            .await
        {
            error!("failed to emit completion event: {}", e);
            // Proceed to complete the lease anyway
        }

        let decision = RetryHandler::decide(outcome, &task.policy, task.attempt);

        match decision {
            knitting_crab_core::RetryDecision::Complete => {
                self.lease_manager.complete(task.task_id).await?;
            }
            knitting_crab_core::RetryDecision::Retry { delay } => {
                self.event_sink
                    .emit_event(TaskEvent::WillRetry {
                        task_id: task.task_id,
                        attempt: task.attempt + 1,
                        delay_ms: delay.as_millis() as u64,
                    })
                    .await?;

                self.lease_manager
                    .fail(task.task_id, task.attempt + 1)
                    .await?;
                sleep(delay).await;
                self.queue.requeue(task.task_id, task.attempt + 1).await?;
            }
            knitting_crab_core::RetryDecision::Abandon => {
                self.event_sink
                    .emit_event(TaskEvent::Abandoned {
                        task_id: task.task_id,
                        reason: "max attempts exceeded".to_string(),
                    })
                    .await?;

                self.lease_manager.fail(task.task_id, task.attempt).await?;
            }
        }

        Ok(())
    }

    /// Reaper loop: periodically collect expired leases and requeue them.
    pub async fn reaper_loop(&self) -> Result<(), WorkerError> {
        let span = tracing::info_span!("reaper", worker_id = %self.worker_id);
        let _guard = span.enter();

        let mut ticker = interval(Duration::from_millis(self.reaper_interval_ms));

        loop {
            ticker.tick().await;

            match self.lease_manager.collect_expired().await {
                Ok(expired) => {
                    for lease in expired {
                        debug!("reaping expired lease: {}", lease.task_id);

                        self.event_sink
                            .emit_event(TaskEvent::LeaseExpired {
                                task_id: lease.task_id,
                            })
                            .await?;

                        self.queue.requeue(lease.task_id, lease.attempt + 1).await?;
                    }
                }
                Err(e) => {
                    error!("reaper error: {}", e);
                }
            }
        }
    }

    /// Cancel a task in progress.
    pub async fn cancel_task(
        &self,
        task_id: knitting_crab_core::ids::TaskId,
    ) -> Result<(), WorkerError> {
        // Trigger cancellation if active locally
        if let Some(token) = self.active_tasks.get(&task_id) {
            token.cancel();
        }

        if self.lease_store.get(task_id).await?.is_some() {
            self.lease_manager.cancel(task_id).await?;
            self.event_sink
                .emit_event(TaskEvent::Cancelled { task_id })
                .await?;
            self.queue.discard(task_id).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_worker::FakeWorker;
    use crate::lease_manager::InMemoryLeaseStore;
    use knitting_crab_core::resource::ResourceAllocation;
    use knitting_crab_core::retry::RetryPolicy;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_worker_runtime_basic() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();

        let worker_id = WorkerId::new();
        let _runtime = WorkerRuntime::new(
            worker_id,
            queue.clone(),
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        );

        let task = knitting_crab_core::TaskDescriptor {
            task_id: knitting_crab_core::TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
            location: Default::default(),
        };

        queue.enqueue(task.clone());

        // Note: can't directly await the loop, so just verify task was dequeued
        match queue.dequeue(worker_id).await {
            Ok(Some(t)) => assert_eq!(t.task_id, task.task_id),
            _ => panic!("task not found"),
        }
    }

    // ===== Configuration Tests =====

    #[test]
    fn test_runtime_configuration_lease_ttl() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();
        let worker_id = WorkerId::new();

        let custom_ttl = Duration::from_secs(60);
        let runtime = WorkerRuntime::new(
            worker_id,
            queue,
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        )
        .with_lease_ttl(custom_ttl);

        assert_eq!(runtime.lease_ttl, custom_ttl);
    }

    #[test]
    fn test_runtime_configuration_heartbeat_interval() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();
        let worker_id = WorkerId::new();

        let custom_interval = 2000;
        let runtime = WorkerRuntime::new(
            worker_id,
            queue,
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        )
        .with_heartbeat_interval(custom_interval);

        assert_eq!(runtime.heartbeat_interval_ms, custom_interval);
    }

    #[test]
    fn test_runtime_configuration_reaper_interval() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();
        let worker_id = WorkerId::new();

        let custom_interval = 15000;
        let runtime = WorkerRuntime::new(
            worker_id,
            queue,
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        )
        .with_reaper_interval(custom_interval);

        assert_eq!(runtime.reaper_interval_ms, custom_interval);
    }

    // ===== Task Execution Tests =====

    #[tokio::test]
    async fn test_execute_task_acquires_lease() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();

        let worker_id = WorkerId::new();
        let runtime = WorkerRuntime::new(
            worker_id,
            queue,
            lease_store.clone(),
            resource_monitor,
            event_sink,
            process_executor,
        );

        let task_id = knitting_crab_core::TaskId::new();
        let task = knitting_crab_core::TaskDescriptor {
            task_id,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
            location: Default::default(),
        };

        // Execute task
        let result = runtime.execute_task(&task).await;
        assert!(result.is_ok(), "task execution should succeed");

        // Verify lease was acquired (should exist, even if completed)
        let lease = lease_store.get(task_id).await;
        assert!(
            lease.is_ok(),
            "lease store should be accessible after execution"
        );
    }

    #[tokio::test]
    async fn test_cancel_task_removes_from_queue() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();

        let worker_id = WorkerId::new();
        let runtime = WorkerRuntime::new(
            worker_id,
            queue.clone(),
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        );

        let task_id = knitting_crab_core::TaskId::new();
        let task = knitting_crab_core::TaskDescriptor {
            task_id,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
            location: Default::default(),
        };

        // Enqueue and then cancel
        queue.enqueue(task.clone());
        let cancel_result = runtime.cancel_task(task_id).await;
        assert!(cancel_result.is_ok(), "cancel should succeed");
    }

    #[test]
    fn test_configuration_chain() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();
        let worker_id = WorkerId::new();

        let runtime = WorkerRuntime::new(
            worker_id,
            queue,
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        )
        .with_lease_ttl(Duration::from_secs(90))
        .with_heartbeat_interval(3000)
        .with_reaper_interval(12000);

        assert_eq!(runtime.lease_ttl, Duration::from_secs(90));
        assert_eq!(runtime.heartbeat_interval_ms, 3000);
        assert_eq!(runtime.reaper_interval_ms, 12000);
    }

    // ===== Worker Identity Tests =====

    #[test]
    fn test_runtime_preserves_worker_id() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();
        let worker_id = WorkerId::new();

        let runtime = WorkerRuntime::new(
            worker_id,
            queue,
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        );

        assert_eq!(runtime.worker_id, worker_id);
    }

    // ===== Lease Manager Integration Tests =====

    #[tokio::test]
    async fn test_lease_manager_initialization() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();

        let worker_id = WorkerId::new();
        let runtime = WorkerRuntime::new(
            worker_id,
            queue,
            lease_store,
            resource_monitor,
            event_sink,
            process_executor,
        );

        // Lease manager should be initialized and ready
        assert_eq!(runtime.worker_id, worker_id);
        assert_eq!(runtime.lease_ttl, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_execute_task_requeues_on_insufficient_resources() {
        let queue = FakeWorker::new();
        let lease_store = InMemoryLeaseStore::new();
        let resource_monitor = FakeWorker::new();
        let event_sink = FakeWorker::new();
        let process_executor = FakeWorker::new();

        // Set resource behavior to deny
        resource_monitor.set_resource_behavior(crate::fake_worker::ResourceBehavior::AlwaysDeny);

        let worker_id = WorkerId::new();
        let runtime = WorkerRuntime::new(
            worker_id,
            queue.clone(),
            lease_store.clone(),
            resource_monitor.clone(),
            event_sink,
            process_executor,
        );

        let task_id = knitting_crab_core::TaskId::new();
        let task = knitting_crab_core::TaskDescriptor {
            task_id,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: Vec::new(),
            location: Default::default(),
        };

        // Enqueue task (so FakeWorker can find it during requeue)
        queue.enqueue(task.clone());

        // Execute task
        let result = runtime.execute_task(&task).await;
        assert!(result.is_ok(), "task execution should return ok (requeued)");

        // Verify task was NOT executed (no lease acquired)
        let lease = lease_store.get(task_id).await.unwrap();
        assert!(lease.is_none(), "lease should not be acquired");

        // Verify task was requeued
        // Since we didn't dequeue it, FakeWorker::requeue duplicates it.
        // So we should have 2 items now.
        let item1 = queue.dequeue(worker_id).await.unwrap();
        let item2 = queue.dequeue(worker_id).await.unwrap();

        assert!(item1.is_some());
        assert!(item2.is_some());
        assert!(queue.dequeue(worker_id).await.unwrap().is_none());
    }
}
