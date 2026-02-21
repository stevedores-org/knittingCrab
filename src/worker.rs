use crate::work_item::WorkItem;
use std::fmt;
use std::time::Duration;

/// Result of executing a task.
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub task_id: u64,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub elapsed: Duration,
}

/// A worker that can execute tasks.
pub trait Worker: Send + Sync {
    fn execute(&self, task: &WorkItem) -> impl std::future::Future<Output = TaskResult> + Send;
}

/// Outcome to configure on a FakeWorker.
#[derive(Debug, Clone)]
pub enum FakeOutcome {
    Succeed,
    Fail { exit_code: i32, reason: String },
    Panic,
}

impl fmt::Display for FakeOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FakeOutcome::Succeed => write!(f, "Succeed"),
            FakeOutcome::Fail { exit_code, .. } => write!(f, "Fail({})", exit_code),
            FakeOutcome::Panic => write!(f, "Panic"),
        }
    }
}

/// A fake worker for component testing with configurable latency and outcomes.
pub struct FakeWorker {
    latency: Duration,
    outcomes: std::sync::Mutex<Vec<FakeOutcome>>,
}

impl FakeWorker {
    pub fn new(latency: Duration, outcomes: Vec<FakeOutcome>) -> Self {
        FakeWorker {
            latency,
            outcomes: std::sync::Mutex::new(outcomes),
        }
    }

    /// Creates a FakeWorker that always succeeds.
    pub fn always_succeed(latency: Duration) -> Self {
        Self {
            latency,
            outcomes: std::sync::Mutex::new(vec![]),
        }
    }
}

impl Worker for FakeWorker {
    async fn execute(&self, task: &WorkItem) -> TaskResult {
        tokio::time::sleep(self.latency).await;

        let outcome = {
            let mut outcomes = self.outcomes.lock().unwrap();
            if outcomes.is_empty() {
                FakeOutcome::Succeed
            } else {
                outcomes.remove(0)
            }
        };

        match outcome {
            FakeOutcome::Succeed => TaskResult {
                task_id: task.id,
                success: true,
                exit_code: Some(0),
                reason: None,
                elapsed: self.latency,
            },
            FakeOutcome::Fail { exit_code, reason } => TaskResult {
                task_id: task.id,
                success: false,
                exit_code: Some(exit_code),
                reason: Some(reason),
                elapsed: self.latency,
            },
            FakeOutcome::Panic => {
                panic!("FakeWorker configured to panic on task {}", task.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::Priority;

    fn make_task(id: u64) -> WorkItem {
        WorkItem::new_simple(
            id,
            format!("goal{}", id),
            "repo",
            "main",
            Priority::Batch,
            vec![],
            1,
            512,
            false,
            None,
            3,
        )
    }

    #[tokio::test]
    async fn fake_worker_succeed() {
        let worker = FakeWorker::always_succeed(Duration::from_millis(1));
        let result = worker.execute(&make_task(1)).await;
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn fake_worker_fail() {
        let worker = FakeWorker::new(
            Duration::from_millis(1),
            vec![FakeOutcome::Fail {
                exit_code: 1,
                reason: "boom".into(),
            }],
        );
        let result = worker.execute(&make_task(1)).await;
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(1));
    }

    #[tokio::test]
    #[should_panic(expected = "FakeWorker configured to panic")]
    async fn fake_worker_panic() {
        let worker = FakeWorker::new(Duration::from_millis(1), vec![FakeOutcome::Panic]);
        worker.execute(&make_task(1)).await;
    }
}
