use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use knitting_crab_core::error::CoreError;
use knitting_crab_core::event::{LogLine, TaskEvent};
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_core::retry::ExitOutcome;
use knitting_crab_core::traits::{EventSink, Queue, ResourceMonitor, TaskDescriptor};

use crate::error::WorkerError;

/// Behavior for a fake worker task.
#[derive(Debug, Clone)]
pub enum FakeBehavior {
    Succeed { delay_ms: u64 },
    Fail { exit_code: i32, delay_ms: u64 },
    Hang,
    Crash,
}

/// A test double that implements all traits and doesn't spawn real processes.
#[derive(Clone)]
pub struct FakeWorker {
    queue: Arc<Mutex<VecDeque<TaskDescriptor>>>,
    events: Arc<Mutex<Vec<TaskEvent>>>,
    logs: Arc<Mutex<Vec<LogLine>>>,
    behaviors: Arc<Mutex<std::collections::HashMap<TaskId, FakeBehavior>>>,
    #[allow(dead_code)]
    worker_id: WorkerId,
}

impl FakeWorker {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            events: Arc::new(Mutex::new(Vec::new())),
            logs: Arc::new(Mutex::new(Vec::new())),
            behaviors: Arc::new(Mutex::new(std::collections::HashMap::new())),
            worker_id: WorkerId::new(),
        }
    }

    pub fn enqueue(&self, task: TaskDescriptor) {
        let mut q = self.queue.lock().unwrap();
        q.push_back(task);
    }

    pub fn set_behavior(&self, task_id: TaskId, behavior: FakeBehavior) {
        let mut behaviors = self.behaviors.lock().unwrap();
        behaviors.insert(task_id, behavior);
    }

    pub fn drain_events(&self) -> Vec<TaskEvent> {
        self.events.lock().unwrap().drain(..).collect()
    }

    pub fn drain_logs(&self) -> Vec<LogLine> {
        self.logs.lock().unwrap().drain(..).collect()
    }

    pub fn get_behavior(&self, task_id: TaskId) -> Option<FakeBehavior> {
        let behaviors = self.behaviors.lock().unwrap();
        behaviors.get(&task_id).cloned()
    }
}

impl Default for FakeWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Queue for FakeWorker {
    async fn dequeue(&self, _worker_id: WorkerId) -> Result<Option<TaskDescriptor>, CoreError> {
        let mut q = self.queue.lock().unwrap();
        Ok(q.pop_front())
    }

    async fn requeue(&self, task_id: TaskId, attempt: u32) -> Result<(), CoreError> {
        let mut q = self.queue.lock().unwrap();
        for task in q.iter_mut() {
            if task.task_id == task_id {
                task.attempt = attempt;
                let new_task = task.clone();
                q.push_back(new_task);
                return Ok(());
            }
        }
        Ok(())
    }

    async fn discard(&self, _task_id: TaskId) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl ResourceMonitor for FakeWorker {
    async fn can_allocate(&self, _allocation: &ResourceAllocation) -> Result<bool, CoreError> {
        Ok(true)
    }

    async fn allocate(&self, _allocation: &ResourceAllocation) -> Result<(), CoreError> {
        Ok(())
    }

    async fn release(&self, _allocation: &ResourceAllocation) -> Result<(), CoreError> {
        Ok(())
    }
}

#[async_trait]
impl EventSink for FakeWorker {
    async fn emit_event(&self, event: TaskEvent) -> Result<(), CoreError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    async fn emit_log(&self, log: LogLine) -> Result<(), CoreError> {
        self.logs.lock().unwrap().push(log);
        Ok(())
    }
}

/// Fake process executor for testing.
#[async_trait]
impl crate::worker_runtime::ProcessExecutor for FakeWorker {
    async fn execute(
        &self,
        params: crate::process::SpawnParams,
        _sink: Arc<dyn EventSink>,
        mut cancel_guard: crate::cancel_token::CancelGuard,
    ) -> Result<ExitOutcome, WorkerError> {
        if cancel_guard.is_cancelled() {
            return Ok(ExitOutcome::Success);
        }

        let behavior = self
            .get_behavior(params.task_id)
            .unwrap_or(FakeBehavior::Succeed { delay_ms: 10 });

        match behavior {
            FakeBehavior::Succeed { delay_ms } => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {
                        Ok(ExitOutcome::Success)
                    }
                    _ = cancel_guard.cancelled() => {
                        Ok(ExitOutcome::KilledBySignal(15))
                    }
                }
            }
            FakeBehavior::Fail {
                exit_code,
                delay_ms,
            } => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {
                        Ok(ExitOutcome::FailedWithCode(exit_code))
                    }
                    _ = cancel_guard.cancelled() => {
                        Ok(ExitOutcome::KilledBySignal(15))
                    }
                }
            }
            FakeBehavior::Hang => {
                cancel_guard.cancelled().await;
                Ok(ExitOutcome::KilledBySignal(15))
            }
            FakeBehavior::Crash => Ok(ExitOutcome::FailedWithCode(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel_token::CancelToken;
    use crate::worker_runtime::ProcessExecutor;

    #[test]
    fn test_fake_worker_queue() {
        let worker = FakeWorker::new();
        let task = TaskDescriptor {
            task_id: TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: knitting_crab_core::RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
        };

        worker.enqueue(task.clone());
        let queue = worker.queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
    }

    #[tokio::test]
    async fn test_fake_worker_events() {
        let worker = FakeWorker::new();
        let event = TaskEvent::Started {
            task_id: TaskId::new(),
            pid: 1234,
        };

        worker.emit_event(event).await.unwrap();
        let events = worker.drain_events();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn log_stream_is_ordered_and_lossless() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();

        for i in 0..10 {
            let log = knitting_crab_core::LogLine::new(
                task_id,
                i,
                knitting_crab_core::LogSource::Stdout,
                format!("log line {}", i),
            );
            worker.emit_log(log).await.unwrap();
        }

        let logs = worker.drain_logs();
        assert_eq!(logs.len(), 10);

        for (i, log) in logs.iter().enumerate() {
            assert_eq!(log.seq, i as u64);
            assert_eq!(log.content, format!("log line {}", i));
        }
    }

    // ===== Behavior Configuration Tests =====

    #[tokio::test]
    async fn test_fake_worker_succeed_behavior() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();

        worker.set_behavior(task_id, FakeBehavior::Succeed { delay_ms: 10 });

        let behavior = worker.get_behavior(task_id);
        assert!(matches!(
            behavior,
            Some(FakeBehavior::Succeed { delay_ms: 10 })
        ));
    }

    #[tokio::test]
    async fn test_fake_worker_fail_behavior() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();

        worker.set_behavior(
            task_id,
            FakeBehavior::Fail {
                exit_code: 42,
                delay_ms: 20,
            },
        );

        let behavior = worker.get_behavior(task_id);
        assert!(matches!(
            behavior,
            Some(FakeBehavior::Fail {
                exit_code: 42,
                delay_ms: 20
            })
        ));
    }

    #[tokio::test]
    async fn test_fake_worker_hang_behavior() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();

        worker.set_behavior(task_id, FakeBehavior::Hang);

        let behavior = worker.get_behavior(task_id);
        assert!(matches!(behavior, Some(FakeBehavior::Hang)));
    }

    #[tokio::test]
    async fn test_fake_worker_crash_behavior() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();

        worker.set_behavior(task_id, FakeBehavior::Crash);

        let behavior = worker.get_behavior(task_id);
        assert!(matches!(behavior, Some(FakeBehavior::Crash)));
    }

    // ===== ProcessExecutor Tests =====

    #[tokio::test]
    async fn test_process_executor_succeed_behavior() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();
        let params = crate::process::SpawnParams {
            task_id,
            command: vec!["test".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
        };

        worker.set_behavior(task_id, FakeBehavior::Succeed { delay_ms: 5 });

        let (_cancel_token, cancel_guard) = CancelToken::new();
        let sink = Arc::new(worker.clone());

        let outcome = ProcessExecutor::execute(&worker, params, sink, cancel_guard)
            .await
            .unwrap();

        assert_eq!(outcome, ExitOutcome::Success);
    }

    #[tokio::test]
    async fn test_process_executor_fail_behavior() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();
        let params = crate::process::SpawnParams {
            task_id,
            command: vec!["test".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
        };

        worker.set_behavior(
            task_id,
            FakeBehavior::Fail {
                exit_code: 42,
                delay_ms: 5,
            },
        );

        let (_cancel_token, cancel_guard) = CancelToken::new();
        let sink = Arc::new(worker.clone());

        let outcome = ProcessExecutor::execute(&worker, params, sink, cancel_guard)
            .await
            .unwrap();

        assert_eq!(outcome, ExitOutcome::FailedWithCode(42));
    }

    #[tokio::test]
    async fn test_process_executor_crash_behavior() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();
        let params = crate::process::SpawnParams {
            task_id,
            command: vec!["test".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
        };

        worker.set_behavior(task_id, FakeBehavior::Crash);

        let (_cancel_token, cancel_guard) = CancelToken::new();
        let sink = Arc::new(worker.clone());

        let outcome = ProcessExecutor::execute(&worker, params, sink, cancel_guard)
            .await
            .unwrap();

        assert_eq!(outcome, ExitOutcome::FailedWithCode(1));
    }

    #[tokio::test]
    async fn test_process_executor_hang_behavior_with_cancellation() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();
        let params = crate::process::SpawnParams {
            task_id,
            command: vec!["test".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
        };

        worker.set_behavior(task_id, FakeBehavior::Hang);

        let (cancel_token, cancel_guard) = CancelToken::new();
        let worker_clone = worker.clone();
        let sink = Arc::new(worker.clone());

        // Spawn the execution and cancel it
        let exec_task: tokio::task::JoinHandle<Result<ExitOutcome, WorkerError>> =
            tokio::spawn(async move {
                ProcessExecutor::execute(&worker_clone, params, sink, cancel_guard).await
            });

        // Give it time to start hanging
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Cancel the task
        cancel_token.cancel();

        let outcome = exec_task.await.unwrap().unwrap();
        assert_eq!(outcome, ExitOutcome::KilledBySignal(15));
    }

    #[tokio::test]
    async fn test_process_executor_cancel_during_succeed() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();
        let params = crate::process::SpawnParams {
            task_id,
            command: vec!["test".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
        };

        // Long delay so cancellation can interrupt
        worker.set_behavior(task_id, FakeBehavior::Succeed { delay_ms: 1000 });

        let (cancel_token, cancel_guard) = CancelToken::new();
        let worker_clone = worker.clone();
        let sink = Arc::new(worker.clone());

        let exec_task: tokio::task::JoinHandle<Result<ExitOutcome, WorkerError>> =
            tokio::spawn(async move {
                ProcessExecutor::execute(&worker_clone, params, sink, cancel_guard).await
            });

        // Give it time to start
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Cancel the task
        cancel_token.cancel();

        let outcome = exec_task.await.unwrap().unwrap();
        assert_eq!(outcome, ExitOutcome::KilledBySignal(15));
    }

    #[tokio::test]
    async fn test_process_executor_already_cancelled() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();
        let params = crate::process::SpawnParams {
            task_id,
            command: vec!["test".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
        };

        worker.set_behavior(task_id, FakeBehavior::Hang);

        let (cancel_token, cancel_guard) = CancelToken::new();

        // Pre-cancel
        cancel_token.cancel();

        // Guard should report already cancelled
        if cancel_guard.is_cancelled() {
            let sink = Arc::new(worker.clone());
            let outcome = ProcessExecutor::execute(&worker, params, sink, cancel_guard)
                .await
                .unwrap();

            // If already cancelled, returns Success
            assert_eq!(outcome, ExitOutcome::Success);
        }
    }

    // ===== Queue Tests =====

    #[tokio::test]
    async fn test_dequeue_returns_none_when_empty() {
        let worker = FakeWorker::new();
        let result = worker.dequeue(WorkerId::new()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_dequeue_fifo_order() {
        let worker = FakeWorker::new();
        let task1 = TaskDescriptor {
            task_id: TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: knitting_crab_core::RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
        };

        let mut task2 = task1.clone();
        task2.task_id = TaskId::new();

        worker.enqueue(task1.clone());
        worker.enqueue(task2.clone());

        let dequeued1 = worker.dequeue(WorkerId::new()).await.unwrap().unwrap();
        let dequeued2 = worker.dequeue(WorkerId::new()).await.unwrap().unwrap();

        assert_eq!(dequeued1.task_id, task1.task_id);
        assert_eq!(dequeued2.task_id, task2.task_id);
    }

    #[tokio::test]
    async fn test_requeue_updates_attempt() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();
        let task = TaskDescriptor {
            task_id,
            command: vec!["echo".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: knitting_crab_core::RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
        };

        worker.enqueue(task);

        // Requeue with attempt 1
        worker.requeue(task_id, 1).await.unwrap();

        // The requeued task should be at the back with updated attempt
        let dequeued = worker.dequeue(WorkerId::new()).await.unwrap().unwrap();
        assert_eq!(dequeued.task_id, task_id);
        assert_eq!(dequeued.attempt, 1);
    }

    #[tokio::test]
    async fn test_discard_succeeds() {
        let worker = FakeWorker::new();
        let task_id = TaskId::new();

        let result = worker.discard(task_id).await;
        assert!(result.is_ok());
    }

    // ===== Resource Monitor Tests =====

    #[tokio::test]
    async fn test_can_allocate_always_true() {
        let worker = FakeWorker::new();
        let allocation = ResourceAllocation::default();

        let result = worker.can_allocate(&allocation).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn test_allocate_succeeds() {
        let worker = FakeWorker::new();
        let allocation = ResourceAllocation::default();

        let result = worker.allocate(&allocation).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_release_succeeds() {
        let worker = FakeWorker::new();
        let allocation = ResourceAllocation::default();

        let result = worker.release(&allocation).await;
        assert!(result.is_ok());
    }

    // ===== Draining Tests =====

    #[tokio::test]
    async fn test_drain_events_clears_queue() {
        let worker = FakeWorker::new();
        let event = TaskEvent::Started {
            task_id: TaskId::new(),
            pid: 1234,
        };

        worker.emit_event(event).await.unwrap();
        let events1 = worker.drain_events();
        let events2 = worker.drain_events();

        assert_eq!(events1.len(), 1);
        assert_eq!(events2.len(), 0);
    }

    #[tokio::test]
    async fn test_drain_logs_clears_queue() {
        let worker = FakeWorker::new();
        let log = knitting_crab_core::LogLine::new(
            TaskId::new(),
            0,
            knitting_crab_core::LogSource::Stdout,
            "test".to_string(),
        );

        worker.emit_log(log).await.unwrap();
        let logs1 = worker.drain_logs();
        let logs2 = worker.drain_logs();

        assert_eq!(logs1.len(), 1);
        assert_eq!(logs2.len(), 0);
    }

    #[test]
    fn test_fake_worker_default() {
        let worker = FakeWorker::default();
        let queue = worker.queue.lock().unwrap();
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_fake_worker_clone_shares_state() {
        let worker1 = FakeWorker::new();
        let worker2 = worker1.clone();

        let task = TaskDescriptor {
            task_id: TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: knitting_crab_core::RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
        };

        worker1.enqueue(task.clone());

        // worker2 should see the same task (shared state)
        let queue = worker2.queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
    }
}
