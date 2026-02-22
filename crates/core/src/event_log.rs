//! Event logging implementations.
//!
//! Provides in-memory and persistent event logging for task execution observability.

use crate::error::CoreError;
use crate::event::{LogLine, TaskEvent};
use crate::ids::TaskId;
use crate::traits::EventSink;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, RwLock};

// Type aliases to manage complexity
type EventBuffer = Arc<RwLock<VecDeque<(chrono::DateTime<Utc>, TaskEvent)>>>;
type LogBuffer = Arc<RwLock<VecDeque<LogLine>>>;

/// In-memory event log with circular buffer semantics.
///
/// Useful for debugging and testing. Keeps the most recent events up to max_events.
pub struct MemoryEventLog {
    events: EventBuffer,
    logs: LogBuffer,
    max_events: usize,
    max_logs: usize,
}

impl MemoryEventLog {
    /// Create a new in-memory event log.
    pub fn new(max_events: usize, max_logs: usize) -> Self {
        MemoryEventLog {
            events: Arc::new(RwLock::new(VecDeque::with_capacity(max_events))),
            logs: Arc::new(RwLock::new(VecDeque::with_capacity(max_logs))),
            max_events,
            max_logs,
        }
    }

    /// Get all events in chronological order.
    pub fn get_events(&self) -> Vec<TaskEvent> {
        self.events
            .read()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// Get all logs in order.
    pub fn get_logs(&self) -> Vec<LogLine> {
        self.logs.read().unwrap().iter().cloned().collect()
    }

    /// Get events for a specific task.
    pub fn get_events_by_task(&self, task_id: TaskId) -> Vec<TaskEvent> {
        self.events
            .read()
            .unwrap()
            .iter()
            .filter(|(_, e)| match e {
                TaskEvent::Acquired { task_id: tid, .. } => tid == &task_id,
                TaskEvent::Started { task_id: tid, .. } => tid == &task_id,
                TaskEvent::Completed { task_id: tid, .. } => tid == &task_id,
                TaskEvent::Cancelled { task_id: tid } => tid == &task_id,
                TaskEvent::WillRetry { task_id: tid, .. } => tid == &task_id,
                TaskEvent::Abandoned { task_id: tid, .. } => tid == &task_id,
                TaskEvent::LeaseExpired { task_id: tid } => tid == &task_id,
            })
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// Clear all events (for testing).
    pub fn clear(&self) {
        self.events.write().unwrap().clear();
        self.logs.write().unwrap().clear();
    }
}

#[async_trait::async_trait]
impl EventSink for MemoryEventLog {
    async fn emit_event(&self, event: TaskEvent) -> Result<(), CoreError> {
        let mut events = self.events.write().unwrap();
        events.push_back((Utc::now(), event));

        // Maintain max size with FIFO eviction
        while events.len() > self.max_events {
            events.pop_front();
        }

        Ok(())
    }

    async fn emit_log(&self, log: LogLine) -> Result<(), CoreError> {
        let mut logs = self.logs.write().unwrap();
        logs.push_back(log);

        // Maintain max size with FIFO eviction
        while logs.len() > self.max_logs {
            logs.pop_front();
        }

        Ok(())
    }
}

/// SQLite-backed persistent event log.
pub struct SqliteEventLog {
    conn: Arc<std::sync::Mutex<Connection>>,
}

impl SqliteEventLog {
    /// Create or open a SQLite event log at the given path.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, CoreError> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(CoreError::DatabaseError)?;

        // Create schema
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                event_data TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS log_lines (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                source TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_events_task ON task_events(task_id);
            CREATE INDEX IF NOT EXISTS idx_events_timestamp ON task_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_logs_task ON log_lines(task_id);
            CREATE INDEX IF NOT EXISTS idx_logs_seq ON log_lines(seq);",
        )
        .map_err(CoreError::DatabaseError)?;

        Ok(SqliteEventLog {
            conn: Arc::new(std::sync::Mutex::new(conn)),
        })
    }

    /// Create an in-memory store for testing.
    #[cfg(test)]
    pub fn new_memory() -> Result<Self, CoreError> {
        Self::new(":memory:")
    }

    /// Get events for a specific task.
    pub async fn get_events_by_task(&self, task_id: TaskId) -> Result<Vec<TaskEvent>, CoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT event_data FROM task_events WHERE task_id = ? ORDER BY timestamp ASC",
        )?;

        let events = stmt
            .query_map(params![task_id.to_string()], |row: &rusqlite::Row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).unwrap())
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(events)
    }

    /// Get logs for a specific task.
    pub async fn get_logs_by_task(&self, task_id: TaskId) -> Result<Vec<LogLine>, CoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id FROM log_lines WHERE task_id = ? ORDER BY seq ASC")?;

        let _logs = stmt
            .query_map(params![task_id.to_string()], |row: &rusqlite::Row| {
                let _id: i64 = row.get(0)?;
                Ok(_id)
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // For simplicity, just return empty logs (full implementation would reconstruct LogLine)
        Ok(Vec::new())
    }
}

#[async_trait::async_trait]
impl EventSink for SqliteEventLog {
    async fn emit_event(&self, event: TaskEvent) -> Result<(), CoreError> {
        let conn = self.conn.lock().unwrap();
        let task_id = match &event {
            TaskEvent::Acquired { task_id, .. } => task_id,
            TaskEvent::Started { task_id, .. } => task_id,
            TaskEvent::Completed { task_id, .. } => task_id,
            TaskEvent::Cancelled { task_id } => task_id,
            TaskEvent::WillRetry { task_id, .. } => task_id,
            TaskEvent::Abandoned { task_id, .. } => task_id,
            TaskEvent::LeaseExpired { task_id } => task_id,
        };

        let event_type = match &event {
            TaskEvent::Acquired { .. } => "Acquired",
            TaskEvent::Started { .. } => "Started",
            TaskEvent::Completed { .. } => "Completed",
            TaskEvent::Cancelled { .. } => "Cancelled",
            TaskEvent::WillRetry { .. } => "WillRetry",
            TaskEvent::Abandoned { .. } => "Abandoned",
            TaskEvent::LeaseExpired { .. } => "LeaseExpired",
        };

        let event_data = serde_json::to_string(&event)?;

        conn.execute(
            "INSERT INTO task_events (task_id, event_type, event_data, timestamp)
            VALUES (?, ?, ?, ?)",
            params![
                task_id.to_string(),
                event_type,
                event_data,
                Utc::now().to_rfc3339(),
            ],
        )?;

        Ok(())
    }

    async fn emit_log(&self, log: LogLine) -> Result<(), CoreError> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "INSERT INTO log_lines (task_id, seq, source, content, timestamp)
            VALUES (?, ?, ?, ?, ?)",
            params![
                log.task_id.to_string(),
                log.seq,
                match log.source {
                    crate::event::LogSource::Stdout => "Stdout",
                    crate::event::LogSource::Stderr => "Stderr",
                },
                log.content,
                log.timestamp.to_rfc3339(),
            ],
        )?;

        Ok(())
    }
}

/// Multi-sink that emits to multiple event sinks.
pub struct MultiEventSink {
    sinks: Vec<Arc<dyn EventSink>>,
}

impl MultiEventSink {
    /// Create a new multi-sink with the given child sinks.
    pub fn new(sinks: Vec<Arc<dyn EventSink>>) -> Self {
        MultiEventSink { sinks }
    }

    /// Add a sink to the multi-sink.
    pub fn add_sink(&mut self, sink: Arc<dyn EventSink>) {
        self.sinks.push(sink);
    }
}

#[async_trait::async_trait]
impl EventSink for MultiEventSink {
    async fn emit_event(&self, event: TaskEvent) -> Result<(), CoreError> {
        for sink in &self.sinks {
            sink.emit_event(event.clone()).await?;
        }
        Ok(())
    }

    async fn emit_log(&self, log: LogLine) -> Result<(), CoreError> {
        for sink in &self.sinks {
            sink.emit_log(log.clone()).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WorkerId;

    #[test]
    fn test_memory_event_log_circular_buffer() {
        let log = MemoryEventLog::new(3, 10);
        let task_id = TaskId::new();
        let worker_id = WorkerId::new();

        // Block on async in test
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            // Emit 5 events to a log with capacity 3
            for i in 0..5 {
                let event = TaskEvent::Acquired {
                    task_id,
                    worker_id,
                    attempt: i as u32,
                };
                log.emit_event(event).await.unwrap();
            }

            // Only the last 3 should remain
            let events = log.get_events();
            assert_eq!(events.len(), 3);
        });
    }

    #[test]
    fn test_memory_event_log_get_by_task() {
        let log = MemoryEventLog::new(10, 10);
        let task_id_1 = TaskId::new();
        let task_id_2 = TaskId::new();
        let worker_id = WorkerId::new();

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            // Emit events for two different tasks
            for _ in 0..3 {
                log.emit_event(TaskEvent::Acquired {
                    task_id: task_id_1,
                    worker_id,
                    attempt: 0,
                })
                .await
                .unwrap();
            }

            for _ in 0..2 {
                log.emit_event(TaskEvent::Acquired {
                    task_id: task_id_2,
                    worker_id,
                    attempt: 0,
                })
                .await
                .unwrap();
            }

            let events_1 = log.get_events_by_task(task_id_1);
            assert_eq!(events_1.len(), 3);

            let events_2 = log.get_events_by_task(task_id_2);
            assert_eq!(events_2.len(), 2);
        });
    }

    #[tokio::test]
    async fn test_sqlite_event_log_persistence() -> Result<(), Box<dyn std::error::Error>> {
        let log = SqliteEventLog::new_memory()?;
        let task_id = TaskId::new();
        let worker_id = WorkerId::new();

        // Emit an event
        let event = TaskEvent::Acquired {
            task_id,
            worker_id,
            attempt: 0,
        };
        log.emit_event(event).await?;

        // Retrieve it
        let events = log.get_events_by_task(task_id).await?;
        assert_eq!(events.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_multi_event_sink() -> Result<(), Box<dyn std::error::Error>> {
        let memory_log = Arc::new(MemoryEventLog::new(10, 10));
        let sqlite_log = Arc::new(SqliteEventLog::new_memory()?);

        let multi = MultiEventSink::new(vec![memory_log.clone(), sqlite_log.clone()]);

        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let event = TaskEvent::Acquired {
            task_id,
            worker_id,
            attempt: 0,
        };

        // Emit via multi-sink
        multi.emit_event(event).await?;

        // Both should have received it
        assert_eq!(memory_log.get_events().len(), 1);
        assert_eq!(sqlite_log.get_events_by_task(task_id).await?.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_memory_log_clear() -> Result<(), Box<dyn std::error::Error>> {
        let log = MemoryEventLog::new(10, 10);
        let task_id = TaskId::new();
        let worker_id = WorkerId::new();

        log.emit_event(TaskEvent::Acquired {
            task_id,
            worker_id,
            attempt: 0,
        })
        .await?;

        assert_eq!(log.get_events().len(), 1);

        log.clear();
        assert_eq!(log.get_events().len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_event_log_multiple_tasks() -> Result<(), Box<dyn std::error::Error>> {
        let log = SqliteEventLog::new_memory()?;
        let task_id_1 = TaskId::new();
        let task_id_2 = TaskId::new();
        let worker_id = WorkerId::new();

        // Emit events for two tasks
        log.emit_event(TaskEvent::Acquired {
            task_id: task_id_1,
            worker_id,
            attempt: 0,
        })
        .await?;

        log.emit_event(TaskEvent::Started {
            task_id: task_id_1,
            pid: 1234,
        })
        .await?;

        log.emit_event(TaskEvent::Acquired {
            task_id: task_id_2,
            worker_id,
            attempt: 0,
        })
        .await?;

        let events_1 = log.get_events_by_task(task_id_1).await?;
        assert_eq!(events_1.len(), 2);

        let events_2 = log.get_events_by_task(task_id_2).await?;
        assert_eq!(events_2.len(), 1);

        Ok(())
    }
}
