use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::ids::{LeaseId, TaskId, WorkerId};

/// The state of a lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeaseState {
    /// Lease is active and the task is running.
    Active,

    /// Task completed successfully.
    Completed,

    /// Task failed with given number of attempts.
    Failed { attempts: u32 },

    /// Lease expired without renewal.
    Expired,

    /// Lease was cancelled.
    Cancelled,
}

/// A lease grants a worker exclusive execution rights to a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    pub lease_id: LeaseId,
    pub task_id: TaskId,
    pub worker_id: WorkerId,
    pub state: LeaseState,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub attempt: u32,
}

impl Lease {
    /// Create a new active lease.
    pub fn new(task_id: TaskId, worker_id: WorkerId, ttl: Duration, attempt: u32) -> Self {
        let now = Utc::now();
        Self {
            lease_id: LeaseId::new(),
            task_id,
            worker_id,
            state: LeaseState::Active,
            acquired_at: now,
            expires_at: now + chrono::Duration::from_std(ttl).unwrap_or_default(),
            attempt,
        }
    }

    /// Check if this lease has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Extend the lease by renewing its expiration time.
    pub fn renew(&mut self, ttl: Duration) {
        self.expires_at = Utc::now() + chrono::Duration::from_std(ttl).unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_lease_is_active() {
        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let lease = Lease::new(task_id, worker_id, Duration::from_secs(1), 0);

        assert_eq!(lease.task_id, task_id);
        assert_eq!(lease.worker_id, worker_id);
        assert_eq!(lease.state, LeaseState::Active);
        assert_eq!(lease.attempt, 0);
        assert!(!lease.is_expired());
    }

    #[test]
    fn test_lease_expiry() {
        let task_id = TaskId::new();
        let worker_id = WorkerId::new();
        let mut lease = Lease::new(task_id, worker_id, Duration::from_millis(1), 0);

        std::thread::sleep(Duration::from_millis(10));
        assert!(lease.is_expired());

        lease.renew(Duration::from_secs(10));
        assert!(!lease.is_expired());
    }
}
