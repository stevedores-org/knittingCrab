//! Persistent lease store using SQLite.
//!
//! Saves lease state to disk so tasks can be recovered after scheduler crashes.

use crate::error::CoreError;
use crate::ids::TaskId;
use crate::lease::Lease;
use crate::traits::LeaseStore;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use thiserror::Error;

/// Errors related to lease persistence.
#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("SQLite error: {0}")]
    SqliteError(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("Database initialization failed: {0}")]
    InitError(String),
}

impl From<PersistenceError> for CoreError {
    fn from(err: PersistenceError) -> Self {
        CoreError::Internal(err.to_string())
    }
}

/// SQLite-backed persistent lease store.
pub struct SqliteLeaseStore {
    conn: std::sync::Arc<std::sync::Mutex<Connection>>,
}

impl SqliteLeaseStore {
    /// Create or open a SQLite lease store at the given path.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, PersistenceError> {
        let conn =
            Connection::open(path).map_err(|e| PersistenceError::InitError(e.to_string()))?;

        // Enable foreign keys and WAL mode for better concurrency
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")
            .map_err(|e| PersistenceError::InitError(e.to_string()))?;

        // Create schema
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS leases (
                task_id TEXT PRIMARY KEY,
                lease_id TEXT NOT NULL,
                worker_id TEXT NOT NULL,
                state TEXT NOT NULL,
                acquired_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                json_data TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_expires_at ON leases(expires_at);
            CREATE INDEX IF NOT EXISTS idx_worker_id ON leases(worker_id);",
        )
        .map_err(|e| PersistenceError::InitError(e.to_string()))?;

        Ok(SqliteLeaseStore {
            conn: std::sync::Arc::new(std::sync::Mutex::new(conn)),
        })
    }

    /// Create an in-memory store for testing.
    #[cfg(test)]
    pub fn new_memory() -> Result<Self, PersistenceError> {
        Self::new(":memory:")
    }

    /// Load all active leases from the store (for recovery on startup).
    pub async fn recover_active_leases(&self) -> Result<Vec<Lease>, CoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CoreError::LockPoisoned(format!("lease store: {e}")))?;
        let mut stmt =
            conn.prepare("SELECT json_data FROM leases WHERE expires_at > datetime('now')")?;

        let rows: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        rows.iter()
            .map(|json| {
                serde_json::from_str(json)
                    .map_err(|e| CoreError::Internal(format!("corrupt lease JSON: {e}")))
            })
            .collect()
    }

    /// Find expired leases for cleanup.
    pub async fn find_expired_leases(&self) -> Result<Vec<Lease>, CoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CoreError::LockPoisoned(format!("lease store: {e}")))?;
        let mut stmt =
            conn.prepare("SELECT json_data FROM leases WHERE expires_at <= datetime('now')")?;

        let rows: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        rows.iter()
            .map(|json| {
                serde_json::from_str(json)
                    .map_err(|e| CoreError::Internal(format!("corrupt lease JSON: {e}")))
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl LeaseStore for SqliteLeaseStore {
    async fn insert(&self, lease: Lease) -> Result<(), CoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CoreError::LockPoisoned(format!("lease store: {e}")))?;
        let json_data = serde_json::to_string(&lease)?;
        let state_str = match lease.state {
            crate::lease::LeaseState::Active => "Active",
            crate::lease::LeaseState::Completed => "Completed",
            crate::lease::LeaseState::Failed { .. } => "Failed",
            crate::lease::LeaseState::Expired => "Expired",
            crate::lease::LeaseState::Cancelled => "Cancelled",
        };

        conn.execute(
            "INSERT INTO leases
            (task_id, lease_id, worker_id, state, acquired_at, expires_at, attempt, json_data)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                lease.task_id.to_string(),
                lease.lease_id.to_string(),
                lease.worker_id.to_string(),
                state_str,
                lease.acquired_at.to_rfc3339(),
                lease.expires_at.to_rfc3339(),
                lease.attempt,
                json_data,
            ],
        )
        .map_err(|e| CoreError::Internal(format!("Failed to insert lease: {}", e)))?;

        Ok(())
    }

    async fn get(&self, task_id: TaskId) -> Result<Option<Lease>, CoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CoreError::LockPoisoned(format!("lease store: {e}")))?;
        let mut stmt = conn.prepare("SELECT json_data FROM leases WHERE task_id = ?")?;

        let json: Option<String> = stmt
            .query_row(params![task_id.to_string()], |row| row.get(0))
            .optional()?;

        match json {
            Some(j) => {
                let lease = serde_json::from_str(&j)
                    .map_err(|e| CoreError::Internal(format!("corrupt lease JSON: {e}")))?;
                Ok(Some(lease))
            }
            None => Ok(None),
        }
    }

    async fn update(&self, lease: Lease) -> Result<(), CoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CoreError::LockPoisoned(format!("lease store: {e}")))?;
        let json_data = serde_json::to_string(&lease)?;
        let state_str = match lease.state {
            crate::lease::LeaseState::Active => "Active",
            crate::lease::LeaseState::Completed => "Completed",
            crate::lease::LeaseState::Failed { .. } => "Failed",
            crate::lease::LeaseState::Expired => "Expired",
            crate::lease::LeaseState::Cancelled => "Cancelled",
        };

        conn.execute(
            "UPDATE leases SET state = ?, expires_at = ?, attempt = ?, json_data = ?, updated_at = CURRENT_TIMESTAMP
            WHERE task_id = ?",
            params![
                state_str,
                lease.expires_at.to_rfc3339(),
                lease.attempt,
                json_data,
                lease.task_id.to_string(),
            ],
        )
        .map_err(|e| CoreError::Internal(format!("Failed to update lease: {}", e)))?;

        Ok(())
    }

    async fn remove(&self, task_id: TaskId) -> Result<(), CoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CoreError::LockPoisoned(format!("lease store: {e}")))?;
        conn.execute(
            "DELETE FROM leases WHERE task_id = ?",
            params![task_id.to_string()],
        )?;
        Ok(())
    }

    async fn active_leases(&self) -> Result<Vec<Lease>, CoreError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CoreError::LockPoisoned(format!("lease store: {e}")))?;
        let mut stmt = conn.prepare("SELECT json_data FROM leases WHERE state = 'Active'")?;

        let rows: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        rows.iter()
            .map(|json| {
                serde_json::from_str(json)
                    .map_err(|e| CoreError::Internal(format!("corrupt lease JSON: {e}")))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_insert_and_get() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteLeaseStore::new_memory()?;
        let lease = Lease::new(
            TaskId::new(),
            crate::ids::WorkerId::new(),
            Duration::from_secs(10),
            0,
        );
        let task_id = lease.task_id;

        store.insert(lease.clone()).await?;
        let retrieved = store.get(task_id).await?;

        assert!(retrieved.is_some());
        let retrieved_lease = retrieved.unwrap();
        assert_eq!(retrieved_lease.task_id, task_id);
        assert_eq!(retrieved_lease.state, crate::lease::LeaseState::Active);

        Ok(())
    }

    #[tokio::test]
    async fn test_active_leases() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteLeaseStore::new_memory()?;

        // Create 3 active leases
        for _ in 0..3 {
            let lease = Lease::new(
                TaskId::new(),
                crate::ids::WorkerId::new(),
                Duration::from_secs(100),
                0,
            );
            store.insert(lease).await?;
        }

        let active = store.active_leases().await?;
        assert_eq!(active.len(), 3);

        Ok(())
    }

    #[tokio::test]
    async fn test_remove_lease() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteLeaseStore::new_memory()?;
        let lease = Lease::new(
            TaskId::new(),
            crate::ids::WorkerId::new(),
            Duration::from_secs(10),
            0,
        );
        let task_id = lease.task_id;

        store.insert(lease).await?;
        assert!(store.get(task_id).await?.is_some());

        store.remove(task_id).await?;
        assert!(store.get(task_id).await?.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_update_lease() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteLeaseStore::new_memory()?;
        let mut lease = Lease::new(
            TaskId::new(),
            crate::ids::WorkerId::new(),
            Duration::from_secs(10),
            0,
        );
        let task_id = lease.task_id;

        store.insert(lease.clone()).await?;

        // Renew the lease
        lease.renew(Duration::from_secs(20));
        store.update(lease).await?;

        let retrieved = store.get(task_id).await?;
        assert!(retrieved.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_access() -> Result<(), Box<dyn std::error::Error>> {
        let store = std::sync::Arc::new(SqliteLeaseStore::new_memory()?);
        let mut handles = vec![];

        // Spawn 5 tasks that each create leases
        for _ in 0..5 {
            let store_clone = store.clone();
            let handle = tokio::spawn(async move {
                for _ in 0..3 {
                    let lease = Lease::new(
                        TaskId::new(),
                        crate::ids::WorkerId::new(),
                        Duration::from_secs(10),
                        0,
                    );
                    store_clone.insert(lease).await.unwrap();
                }
            });
            handles.push(handle);
        }

        // Wait for all to complete
        for handle in handles {
            handle.await?;
        }

        let all_leases = store.active_leases().await?;
        assert_eq!(all_leases.len(), 15); // 5 tasks * 3 leases

        Ok(())
    }
}
