//! Priority inversion detection for preventing system deadlocks and unfairness.
//!
//! Detects when high-priority tasks are blocked by low-priority tasks due to
//! lock contention, enabling mitigation strategies like priority inheritance.
//!
//! ## Detection Method
//!
//! The detector tracks task-to-lock ownership mappings and identifies inversions
//! when a high-priority task waits on a lock held by a lower-priority task.

use crate::ids::TaskId;
use crate::priority::Priority;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::RwLock;

/// Record of a detected priority inversion event.
#[derive(Debug, Clone)]
pub struct InversionRecord {
    /// Task ID of the high-priority task waiting.
    pub waiter_id: TaskId,
    /// Priority level of the waiting task.
    pub waiter_priority: Priority,
    /// Task ID of the lower-priority task holding the lock.
    pub blocker_id: TaskId,
    /// Priority level of the blocking task.
    pub blocker_priority: Priority,
    /// Lock identifier that caused the contention.
    pub lock_id: String,
    /// When the inversion was detected.
    pub detected_at: DateTime<Utc>,
    /// How long the inversion has been active (in milliseconds).
    pub duration_ms: u64,
}

/// Tracks task-to-lock ownership relationships for inversion detection.
/// This struct is kept for Phase 2 priority inheritance implementation (see ARCHITECTURE.md).
/// Will be used when implementing lock-aware priority boosting to prevent priority inversion.
#[allow(dead_code)]
struct LockOwnership {
    /// Task holding the lock.
    holder_id: TaskId,
    /// Priority of the holder.
    holder_priority: Priority,
    /// When the lock was acquired.
    acquired_at: DateTime<Utc>,
}

/// Detector for priority inversion events during task execution.
///
/// Maintains runtime lock ownership information to identify when high-priority
/// tasks are blocked by lower-priority tasks, enabling alerts and mitigation.
pub struct PriorityInversionDetector {
    /// Lock ID → ownership info mapping.
    lock_ownership: RwLock<HashMap<String, LockOwnership>>,
    /// Historical inversion records (for analytics).
    inversion_history: RwLock<Vec<InversionRecord>>,
}

impl PriorityInversionDetector {
    /// Create a new priority inversion detector.
    pub fn new() -> Self {
        Self {
            lock_ownership: RwLock::new(HashMap::new()),
            inversion_history: RwLock::new(Vec::new()),
        }
    }

    /// Record that a task has acquired a lock.
    ///
    /// # Errors
    ///
    /// Returns error if lock ownership map is poisoned.
    pub fn record_lock_acquire(
        &self,
        lock_id: impl AsRef<str>,
        task_id: TaskId,
        priority: Priority,
    ) -> Result<(), crate::error::CoreError> {
        let lock_id = lock_id.as_ref();
        let mut ownership = self.lock_ownership.write().map_err(|e| {
            crate::error::CoreError::Internal(format!("lock ownership map poisoned: {}", e))
        })?;

        ownership.insert(
            lock_id.to_string(),
            LockOwnership {
                holder_id: task_id,
                holder_priority: priority,
                acquired_at: Utc::now(),
            },
        );

        Ok(())
    }

    /// Record that a task has released a lock.
    ///
    /// Returns the ID of the task that held the lock.
    pub fn record_lock_release(
        &self,
        lock_id: impl AsRef<str>,
    ) -> Result<Option<TaskId>, crate::error::CoreError> {
        let lock_id = lock_id.as_ref();
        let mut ownership = self.lock_ownership.write().map_err(|e| {
            crate::error::CoreError::Internal(format!("lock ownership map poisoned: {}", e))
        })?;

        Ok(ownership.remove(lock_id).map(|o| o.holder_id))
    }

    /// Record that a high-priority task is blocked on a lock.
    ///
    /// Detects if this creates a priority inversion (waiter has higher priority than holder).
    /// Returns the inversion record if one was detected.
    pub fn record_task_blocks(
        &self,
        waiter_id: TaskId,
        waiter_priority: Priority,
        on_lock_id: impl AsRef<str>,
    ) -> Result<Option<InversionRecord>, crate::error::CoreError> {
        let lock_id = on_lock_id.as_ref();
        let ownership = self.lock_ownership.read().map_err(|e| {
            crate::error::CoreError::Internal(format!("lock ownership map poisoned: {}", e))
        })?;

        if let Some(owner) = ownership.get(lock_id) {
            // Inversion occurs if waiter priority > holder priority
            if waiter_priority > owner.holder_priority {
                let inversion = InversionRecord {
                    waiter_id,
                    waiter_priority,
                    blocker_id: owner.holder_id,
                    blocker_priority: owner.holder_priority,
                    lock_id: lock_id.to_string(),
                    detected_at: Utc::now(),
                    duration_ms: 0, // Would be updated on release
                };

                drop(ownership); // Release read lock before acquiring write lock

                let mut history = self.inversion_history.write().map_err(|e| {
                    crate::error::CoreError::Internal(format!("inversion history poisoned: {}", e))
                })?;
                history.push(inversion.clone());

                return Ok(Some(inversion));
            }
        }

        Ok(None)
    }

    /// Get recent inversion records from the last N milliseconds.
    pub fn recent_inversions(
        &self,
        since_ms: u64,
    ) -> Result<Vec<InversionRecord>, crate::error::CoreError> {
        let history = self.inversion_history.read().map_err(|e| {
            crate::error::CoreError::Internal(format!("inversion history poisoned: {}", e))
        })?;

        let cutoff = Utc::now() - Duration::milliseconds(since_ms as i64);

        Ok(history
            .iter()
            .filter(|r| r.detected_at > cutoff)
            .cloned()
            .collect())
    }

    /// Get total count of detected inversions.
    pub fn inversion_count(&self) -> Result<usize, crate::error::CoreError> {
        let history = self.inversion_history.read().map_err(|e| {
            crate::error::CoreError::Internal(format!("inversion history poisoned: {}", e))
        })?;

        Ok(history.len())
    }

    /// Clear all historical inversion records.
    pub fn clear_history(&self) -> Result<(), crate::error::CoreError> {
        let mut history = self.inversion_history.write().map_err(|e| {
            crate::error::CoreError::Internal(format!("inversion history poisoned: {}", e))
        })?;

        history.clear();
        Ok(())
    }

    /// Clear lock ownership state (for testing/recovery).
    pub fn clear_locks(&self) -> Result<(), crate::error::CoreError> {
        let mut ownership = self.lock_ownership.write().map_err(|e| {
            crate::error::CoreError::Internal(format!("lock ownership map poisoned: {}", e))
        })?;

        ownership.clear();
        Ok(())
    }
}

impl Default for PriorityInversionDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_detector() {
        let detector = PriorityInversionDetector::new();
        assert_eq!(detector.inversion_count().unwrap(), 0);
    }

    #[test]
    fn test_record_lock_acquire_release() {
        let detector = PriorityInversionDetector::new();
        let task_id = TaskId::new();
        let lock_id = "lock1";

        assert!(detector
            .record_lock_acquire(lock_id, task_id, Priority::High)
            .is_ok());

        let released = detector.record_lock_release(lock_id).unwrap();
        assert_eq!(released, Some(task_id));
    }

    #[test]
    fn test_detect_direct_inversion() {
        let detector = PriorityInversionDetector::new();

        let blocker_id = TaskId::new();
        let waiter_id = TaskId::new();
        let lock_id = "critical_lock";

        // Low priority task holds lock
        detector
            .record_lock_acquire(lock_id, blocker_id, Priority::Low)
            .unwrap();

        // High priority task tries to acquire same lock
        let inversion = detector
            .record_task_blocks(waiter_id, Priority::High, lock_id)
            .unwrap();

        assert!(inversion.is_some());
        let inv = inversion.unwrap();
        assert_eq!(inv.waiter_id, waiter_id);
        assert_eq!(inv.waiter_priority, Priority::High);
        assert_eq!(inv.blocker_id, blocker_id);
        assert_eq!(inv.blocker_priority, Priority::Low);
    }

    #[test]
    fn test_no_inversion_if_equal_priority() {
        let detector = PriorityInversionDetector::new();

        let blocker_id = TaskId::new();
        let waiter_id = TaskId::new();
        let lock_id = "lock1";

        // Both tasks have same priority
        detector
            .record_lock_acquire(lock_id, blocker_id, Priority::Normal)
            .unwrap();

        let inversion = detector
            .record_task_blocks(waiter_id, Priority::Normal, lock_id)
            .unwrap();

        assert!(inversion.is_none()); // No inversion for equal priority
    }

    #[test]
    fn test_no_inversion_if_lower_waits_on_higher() {
        let detector = PriorityInversionDetector::new();

        let blocker_id = TaskId::new();
        let waiter_id = TaskId::new();
        let lock_id = "lock1";

        // High priority task holds lock
        detector
            .record_lock_acquire(lock_id, blocker_id, Priority::High)
            .unwrap();

        // Low priority task tries to acquire same lock
        let inversion = detector
            .record_task_blocks(waiter_id, Priority::Low, lock_id)
            .unwrap();

        assert!(inversion.is_none()); // No inversion: lower waits on higher is OK
    }

    #[test]
    fn test_multiple_locks() {
        let detector = PriorityInversionDetector::new();

        let task1 = TaskId::new();
        let task2 = TaskId::new();

        // Two different locks held by different tasks
        detector
            .record_lock_acquire("lock1", task1, Priority::Low)
            .unwrap();
        detector
            .record_lock_acquire("lock2", task2, Priority::Normal)
            .unwrap();

        let inv1 = detector
            .record_task_blocks(TaskId::new(), Priority::High, "lock1")
            .unwrap();
        assert!(inv1.is_some());

        let inv2 = detector
            .record_task_blocks(TaskId::new(), Priority::High, "lock2")
            .unwrap();
        assert!(inv2.is_some());
    }

    #[test]
    fn test_inversion_history() {
        let detector = PriorityInversionDetector::new();

        let task1 = TaskId::new();
        let task2 = TaskId::new();
        let lock_id = "lock1";

        detector
            .record_lock_acquire(lock_id, task1, Priority::Low)
            .unwrap();

        detector
            .record_task_blocks(task2, Priority::Critical, lock_id)
            .unwrap();

        assert_eq!(detector.inversion_count().unwrap(), 1);

        let recent = detector.recent_inversions(5000).unwrap(); // Last 5 seconds
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn test_clear_history() {
        let detector = PriorityInversionDetector::new();

        let task1 = TaskId::new();
        let task2 = TaskId::new();

        detector
            .record_lock_acquire("lock1", task1, Priority::Low)
            .unwrap();
        detector
            .record_task_blocks(task2, Priority::High, "lock1")
            .unwrap();

        assert_eq!(detector.inversion_count().unwrap(), 1);

        detector.clear_history().unwrap();
        assert_eq!(detector.inversion_count().unwrap(), 0);
    }

    #[test]
    fn test_inversion_record_fields() {
        let detector = PriorityInversionDetector::new();

        let blocker = TaskId::new();
        let waiter = TaskId::new();
        let lock_id = "test_lock";

        detector
            .record_lock_acquire(lock_id, blocker, Priority::Low)
            .unwrap();

        let inversion = detector
            .record_task_blocks(waiter, Priority::Critical, lock_id)
            .unwrap()
            .unwrap();

        assert_eq!(inversion.waiter_id, waiter);
        assert_eq!(inversion.waiter_priority, Priority::Critical);
        assert_eq!(inversion.blocker_id, blocker);
        assert_eq!(inversion.blocker_priority, Priority::Low);
        assert_eq!(inversion.lock_id, lock_id);
        assert!(inversion.duration_ms == 0); // Captured at detection time
    }

    #[test]
    fn test_nonexistent_lock() {
        let detector = PriorityInversionDetector::new();

        // Try to detect inversion on non-existent lock
        let inversion = detector
            .record_task_blocks(TaskId::new(), Priority::High, "nonexistent")
            .unwrap();

        assert!(inversion.is_none()); // No holder, no inversion
    }

    #[test]
    fn test_lock_acquire_overwrites_old() {
        let detector = PriorityInversionDetector::new();

        let task1 = TaskId::new();
        let task2 = TaskId::new();
        let lock_id = "lock1";

        // Task1 acquires lock
        detector
            .record_lock_acquire(lock_id, task1, Priority::Normal)
            .unwrap();

        // Task1 releases
        let released = detector.record_lock_release(lock_id).unwrap();
        assert_eq!(released, Some(task1));

        // Task2 acquires same lock
        detector
            .record_lock_acquire(lock_id, task2, Priority::Normal)
            .unwrap();

        let released = detector.record_lock_release(lock_id).unwrap();
        assert_eq!(released, Some(task2));
    }
}
