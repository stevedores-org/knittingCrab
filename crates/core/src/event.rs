use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::TaskId;
use crate::retry::ExitOutcome;

/// A log line emitted by a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub task_id: TaskId,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub source: LogSource,
    pub content: String,
}

/// Source of a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogSource {
    Stdout,
    Stderr,
}

impl LogLine {
    pub fn new(task_id: TaskId, seq: u64, source: LogSource, content: String) -> Self {
        Self {
            task_id,
            seq,
            timestamp: Utc::now(),
            source,
            content,
        }
    }
}

/// An event related to task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskEvent {
    /// Task was acquired by a worker.
    Acquired {
        task_id: TaskId,
        worker_id: crate::ids::WorkerId,
        attempt: u32,
    },

    /// Task execution started.
    Started { task_id: TaskId, pid: u32 },

    /// Task execution completed.
    Completed {
        task_id: TaskId,
        outcome: ExitOutcome,
    },

    /// Task was cancelled.
    Cancelled { task_id: TaskId },

    /// Task will be retried.
    WillRetry {
        task_id: TaskId,
        attempt: u32,
        delay_ms: u64,
    },

    /// Task has been abandoned (no more retries).
    Abandoned { task_id: TaskId, reason: String },

    /// Lease expired.
    LeaseExpired { task_id: TaskId },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_line_creation() {
        let task_id = TaskId::new();
        let log = LogLine::new(task_id, 0, LogSource::Stdout, "hello".to_string());
        assert_eq!(log.task_id, task_id);
        assert_eq!(log.seq, 0);
        assert_eq!(log.source, LogSource::Stdout);
        assert_eq!(log.content, "hello");
    }
}
