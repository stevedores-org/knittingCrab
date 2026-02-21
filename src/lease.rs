use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::error::{Result, SchedulerError};

/// An exclusive, time-bounded grant that allows a worker to run a task.
///
/// Workers must call [`LeaseManager::renew_heartbeat`] periodically to signal
/// liveness; failure to do so marks the lease as overdue.
#[derive(Debug, Clone)]
pub struct Lease {
    /// Unique identifier for this lease (UUID v4).
    pub id: String,
    /// ID of the task this lease covers.
    pub task_id: String,
    /// ID of the worker that holds this lease.
    pub worker_id: String,
    /// Moment the lease was issued.
    pub granted_at: Instant,
    /// Moment after which the lease is considered expired.
    pub expires_at: Instant,
    /// Moment of the most recent heartbeat (or `granted_at` initially).
    pub last_heartbeat: Instant,
    /// How often the worker must heartbeat (in seconds).
    pub heartbeat_interval_secs: u64,
}

impl Lease {
    /// Creates a new lease, starting the clock immediately.
    pub fn new(
        task_id: &str,
        worker_id: &str,
        duration_secs: u64,
        heartbeat_interval_secs: u64,
    ) -> Self {
        let now = Instant::now();
        Self {
            id: Uuid::new_v4().to_string(),
            task_id: task_id.to_owned(),
            worker_id: worker_id.to_owned(),
            granted_at: now,
            expires_at: now + Duration::from_secs(duration_secs),
            last_heartbeat: now,
            heartbeat_interval_secs,
        }
    }

    /// Returns `true` if the lease has passed its expiry time.
    pub fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }

    /// Returns `true` if the last heartbeat is older than the required interval.
    pub fn heartbeat_overdue(&self) -> bool {
        let deadline = self.last_heartbeat + Duration::from_secs(self.heartbeat_interval_secs);
        Instant::now() > deadline
    }

    /// Records a fresh heartbeat, resetting the overdue timer.
    pub fn renew_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    /// Returns the [`Duration`] until the lease expires, or `None` if already
    /// expired.
    pub fn time_remaining(&self) -> Option<Duration> {
        self.expires_at.checked_duration_since(Instant::now())
    }
}

// ── LeaseManager ─────────────────────────────────────────────────────────────

/// Central registry of all active leases and exclusive goal locks.
pub struct LeaseManager {
    leases: Arc<Mutex<HashMap<String, Lease>>>,
    goal_locks: Arc<Mutex<HashSet<String>>>, // "repo:branch:goal_hash"
}

impl LeaseManager {
    /// Creates a new, empty `LeaseManager`.
    pub fn new() -> Self {
        Self {
            leases: Arc::new(Mutex::new(HashMap::new())),
            goal_locks: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Grants a new lease for `task_id` / `worker_id` and stores it.
    pub fn acquire_lease(
        &self,
        task_id: &str,
        worker_id: &str,
        duration_secs: u64,
        heartbeat_secs: u64,
    ) -> Result<Lease> {
        let lease = Lease::new(task_id, worker_id, duration_secs, heartbeat_secs);
        let mut map = self
            .leases
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "leases".to_owned(),
            })?;
        map.insert(lease.id.clone(), lease.clone());
        Ok(lease)
    }

    /// Refreshes the heartbeat timestamp for the lease identified by `lease_id`.
    pub fn renew_heartbeat(&self, lease_id: &str) -> Result<()> {
        let mut map = self
            .leases
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "leases".to_owned(),
            })?;
        let lease = map
            .get_mut(lease_id)
            .ok_or_else(|| SchedulerError::LeaseExpired {
                task_id: lease_id.to_owned(),
            })?;
        lease.renew_heartbeat();
        Ok(())
    }

    /// Removes and discards the lease identified by `lease_id`.
    pub fn release_lease(&self, lease_id: &str) -> Result<()> {
        let mut map = self
            .leases
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "leases".to_owned(),
            })?;
        map.remove(lease_id)
            .ok_or_else(|| SchedulerError::LeaseExpired {
                task_id: lease_id.to_owned(),
            })?;
        Ok(())
    }

    /// Returns the IDs of all leases that have passed their expiry time.
    pub fn check_expired(&self) -> Vec<String> {
        let map = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        map.values()
            .filter(|l| l.is_expired())
            .map(|l| l.id.clone())
            .collect()
    }

    /// Acquires an exclusive lock on the named goal.
    ///
    /// Returns [`SchedulerError::GoalAlreadyLocked`] if another agent holds
    /// the lock.
    pub fn lock_goal(&self, repo: &str, branch: &str, goal_hash: &str) -> Result<()> {
        let key = format!("{repo}:{branch}:{goal_hash}");
        let mut locks = self
            .goal_locks
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "goal_locks".to_owned(),
            })?;
        if locks.contains(&key) {
            return Err(SchedulerError::GoalAlreadyLocked {
                repo: repo.to_owned(),
                branch: branch.to_owned(),
            });
        }
        locks.insert(key);
        Ok(())
    }

    /// Releases the exclusive lock on the named goal (no-op if not held).
    pub fn unlock_goal(&self, repo: &str, branch: &str, goal_hash: &str) {
        let key = format!("{repo}:{branch}:{goal_hash}");
        if let Ok(mut locks) = self.goal_locks.lock() {
            locks.remove(&key);
        }
    }

    /// Returns the number of currently active (not-yet-released) leases.
    pub fn active_lease_count(&self) -> usize {
        let map = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        map.len()
    }
}

impl Default for LeaseManager {
    fn default() -> Self {
        Self::new()
    }
}
