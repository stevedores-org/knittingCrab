use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;

use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::traits::{Queue, TaskDescriptor};

/// A stub scheduler that stores tasks in a VecDeque.
pub struct StubScheduler {
    queue: Mutex<VecDeque<TaskDescriptor>>,
}

impl StubScheduler {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Add a task to the queue.
    pub fn enqueue(&self, task: TaskDescriptor) -> Result<(), CoreError> {
        let mut q = self
            .queue
            .lock()
            .map_err(|e| CoreError::Internal(format!("queue lock poisoned: {}", e)))?;
        q.push_back(task);
        Ok(())
    }
}

impl Default for StubScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Queue for StubScheduler {
    async fn dequeue(&self, _worker_id: WorkerId) -> Result<Option<TaskDescriptor>, CoreError> {
        let mut q = self
            .queue
            .lock()
            .map_err(|e| CoreError::Internal(format!("queue lock poisoned: {}", e)))?;
        Ok(q.pop_front())
    }

    async fn requeue(&self, task_id: TaskId, attempt: u32) -> Result<(), CoreError> {
        let mut q = self
            .queue
            .lock()
            .map_err(|e| CoreError::Internal(format!("queue lock poisoned: {}", e)))?;

        // Find and update the task (stub: just push back with new attempt)
        for task in q.iter_mut() {
            if task.task_id == task_id {
                task.attempt = attempt;
                let new_task = task.clone();
                q.push_back(new_task);
                return Ok(());
            }
        }

        // If not found, that's ok in the stub
        Ok(())
    }

    async fn discard(&self, _task_id: TaskId) -> Result<(), CoreError> {
        // Stub: do nothing
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knitting_crab_core::resource::ResourceAllocation;
    use knitting_crab_core::retry::RetryPolicy;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_stub_scheduler_enqueue_dequeue() {
        let scheduler = StubScheduler::new();
        let task = TaskDescriptor {
            task_id: TaskId::new(),
            command: vec!["echo".to_string(), "hello".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
        };

        scheduler.enqueue(task.clone()).unwrap();

        let worker_id = WorkerId::new();
        let dequeued = scheduler.dequeue(worker_id).await.unwrap();
        assert!(dequeued.is_some());
        assert_eq!(dequeued.unwrap().task_id, task.task_id);
    }
}
