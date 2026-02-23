use async_trait::async_trait;
use dashmap::DashMap;
use tracing;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::traits::GoalLockStore;

/// In-memory goal lock store using DashMap for concurrent access.
///
/// Prevents duplicate agent work on the same goal by allowing only one
/// task to hold a lock for a given goal string.
pub struct InMemoryGoalLockStore {
    locks: DashMap<String, TaskId>,
}

impl InMemoryGoalLockStore {
    /// Create a new empty goal lock store.
    pub fn new() -> Self {
        Self {
            locks: DashMap::new(),
        }
    }
}

impl Default for InMemoryGoalLockStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GoalLockStore for InMemoryGoalLockStore {
    async fn try_acquire(&self, goal: &str, task_id: TaskId) -> Result<(), CoreError> {
        // Only insert if the goal is not already locked
        let entry = self.locks.entry(goal.to_string()).or_insert(task_id);
        if *entry == task_id {
            // We just inserted or already held it
            tracing::info!(goal = %goal, task_id = %task_id, "acquired goal lock");
            Ok(())
        } else {
            // Another task holds the lock
            tracing::debug!(goal = %goal, task_id = %task_id, holder = %*entry, "goal lock conflict");
            Err(CoreError::GoalLockConflict {
                goal: goal.to_string(),
            })
        }
    }

    async fn release(&self, goal: &str, task_id: TaskId) -> Result<(), CoreError> {
        // Only release if the task_id matches the current holder
        // Foreign release is a no-op (entry not removed)
        if let Some(entry) = self.locks.get(goal) {
            if *entry == task_id {
                drop(entry);
                self.locks.remove(goal);
                tracing::info!(goal = %goal, task_id = %task_id, "released goal lock");
            } else {
                tracing::debug!(goal = %goal, task_id = %task_id, holder = %*entry, "ignoring foreign release");
            }
        }
        Ok(())
    }

    async fn holder(&self, goal: &str) -> Result<Option<TaskId>, CoreError> {
        Ok(self.locks.get(goal).map(|entry| *entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn goal_lock_first_acquirer_wins() {
        let store = InMemoryGoalLockStore::new();
        let task_id = TaskId::new();
        let goal = "test-goal";

        let result = store.try_acquire(goal, task_id).await;
        assert!(result.is_ok());
        assert_eq!(store.holder(goal).await.unwrap(), Some(task_id));
    }

    #[tokio::test]
    async fn goal_lock_second_acquirer_fails_with_conflict() {
        let store = InMemoryGoalLockStore::new();
        let task_id_1 = TaskId::new();
        let task_id_2 = TaskId::new();
        let goal = "test-goal";

        let result1 = store.try_acquire(goal, task_id_1).await;
        assert!(result1.is_ok());

        let result2 = store.try_acquire(goal, task_id_2).await;
        assert!(matches!(result2, Err(CoreError::GoalLockConflict { .. })));

        // Verify that task_id_1 is still holding the lock (not task_id_2)
        let holder = store.holder(goal).await.unwrap();
        assert!(holder.is_some(), "Lock should be held");
        assert_ne!(holder, Some(task_id_2), "Task 2 should not hold the lock");
    }

    #[tokio::test]
    async fn goal_lock_release_allows_reacquire() {
        let store = InMemoryGoalLockStore::new();
        let task_id_1 = TaskId::new();
        let task_id_2 = TaskId::new();
        let goal = "test-goal";

        store.try_acquire(goal, task_id_1).await.unwrap();
        store.release(goal, task_id_1).await.unwrap();

        let result = store.try_acquire(goal, task_id_2).await;
        assert!(result.is_ok());
        assert_eq!(store.holder(goal).await.unwrap(), Some(task_id_2));
    }

    #[tokio::test]
    async fn goal_lock_release_by_non_owner_is_noop() {
        let store = InMemoryGoalLockStore::new();
        let task_id_1 = TaskId::new();
        let task_id_2 = TaskId::new();
        let goal = "test-goal";

        store.try_acquire(goal, task_id_1).await.unwrap();
        // Try to release with a different task_id
        store.release(goal, task_id_2).await.unwrap();

        // Lock should still be held by task_id_1
        assert_eq!(store.holder(goal).await.unwrap(), Some(task_id_1));
    }

    #[tokio::test]
    async fn goal_lock_holder_returns_none_when_free() {
        let store = InMemoryGoalLockStore::new();
        let goal = "test-goal";

        let result = store.holder(goal).await.unwrap();
        assert_eq!(result, None);
    }
}
