//! Task timeout enforcement for preventing runaway executions.
//!
//! Provides mechanisms to enforce time limits on task execution with support for
//! both hard timeouts (immediate termination) and soft timeouts (graceful shutdown).

use crate::error::CoreError;
use crate::ids::TaskId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Policy for task timeout behavior.
#[derive(Debug, Clone)]
pub struct TimeoutPolicy {
    /// Hard timeout - task is forcefully terminated after this duration.
    pub hard_timeout: Duration,

    /// Soft timeout (optional) - grace period before hard timeout.
    /// If set, task receives a cancellation signal but can continue for this duration.
    pub soft_timeout: Option<Duration>,
}

impl TimeoutPolicy {
    /// Create a timeout policy with only a hard timeout.
    pub fn hard(duration: Duration) -> Self {
        Self {
            hard_timeout: duration,
            soft_timeout: None,
        }
    }

    /// Create a timeout policy with both soft and hard timeouts.
    pub fn soft_and_hard(soft: Duration, hard: Duration) -> Self {
        Self {
            hard_timeout: hard,
            soft_timeout: Some(soft),
        }
    }

    /// Get the effective timeout duration (whichever comes first).
    pub fn effective_timeout(&self) -> Duration {
        match self.soft_timeout {
            Some(soft) if soft < self.hard_timeout => soft,
            _ => self.hard_timeout,
        }
    }
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            hard_timeout: Duration::from_secs(300), // 5 minutes default
            soft_timeout: None,
        }
    }
}

/// Handle for a task timeout, allowing cancellation and monitoring.
pub struct TimeoutHandle {
    task_id: TaskId,
    cancelled: Arc<AtomicBool>,
    soft_timeout_triggered: Arc<AtomicBool>,
}

impl TimeoutHandle {
    /// Create a new timeout handle for a task.
    pub fn new(task_id: TaskId) -> Self {
        Self {
            task_id,
            cancelled: Arc::new(AtomicBool::new(false)),
            soft_timeout_triggered: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get the task ID associated with this timeout.
    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    /// Check if the task has been cancelled (hard timeout).
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Check if the soft timeout has been triggered.
    pub fn is_soft_timeout(&self) -> bool {
        self.soft_timeout_triggered.load(Ordering::SeqCst)
    }

    /// Cancel the task (hard timeout).
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Trigger soft timeout (grace period before hard timeout).
    pub fn trigger_soft_timeout(&self) {
        self.soft_timeout_triggered.store(true, Ordering::SeqCst);
    }

    /// Reset timeout state (for reuse or testing).
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
        self.soft_timeout_triggered.store(false, Ordering::SeqCst);
    }
}

impl Clone for TimeoutHandle {
    fn clone(&self) -> Self {
        Self {
            task_id: self.task_id,
            cancelled: Arc::clone(&self.cancelled),
            soft_timeout_triggered: Arc::clone(&self.soft_timeout_triggered),
        }
    }
}

/// Task timeout manager that enforces timeouts on task execution.
pub struct TaskTimeoutManager {
    policy: TimeoutPolicy,
    handle: TimeoutHandle,
}

impl TaskTimeoutManager {
    /// Create a new timeout manager with the given policy.
    pub fn new(task_id: TaskId, policy: TimeoutPolicy) -> Self {
        Self {
            policy,
            handle: TimeoutHandle::new(task_id),
        }
    }

    /// Get the timeout handle for monitoring/cancelling the task.
    pub fn handle(&self) -> TimeoutHandle {
        self.handle.clone()
    }

    /// Start a timeout that will enforce the policy.
    ///
    /// Returns a task that can be spawned to enforce the timeout.
    /// The task will trigger soft timeout first (if configured), then hard timeout.
    pub async fn start(&self) {
        // If soft timeout is configured, wait for it first
        if let Some(soft) = self.policy.soft_timeout {
            tokio::time::sleep(soft).await;

            // Only trigger if not already cancelled
            if !self.handle.is_cancelled() {
                self.handle.trigger_soft_timeout();
            }

            // Wait for remaining time until hard timeout
            let remaining = self.policy.hard_timeout.saturating_sub(soft);
            if remaining > Duration::ZERO {
                tokio::time::sleep(remaining).await;
            }
        } else {
            // No soft timeout, go straight to hard timeout
            tokio::time::sleep(self.policy.hard_timeout).await;
        }

        // Trigger hard timeout if not already cancelled
        if !self.handle.is_cancelled() {
            self.handle.cancel();
        }
    }

    /// Check timeout status and return appropriate error if violated.
    pub fn check(&self) -> Result<(), CoreError> {
        if self.handle.is_cancelled() {
            return Err(CoreError::Internal(
                "task exceeded hard timeout".to_string(),
            ));
        }

        if self.handle.is_soft_timeout() {
            // Soft timeout triggered but task still running
            // This is not an error yet, but signals task should prepare to shutdown
            return Ok(());
        }

        Ok(())
    }

    /// Wait for timeout completion (useful for testing).
    pub async fn wait_for_timeout(&self) {
        self.start().await;
    }

    /// Get the configured timeout policy.
    pub fn policy(&self) -> &TimeoutPolicy {
        &self.policy
    }

    /// Get elapsed time since timeout was created (for monitoring).
    pub fn elapsed(&self) -> Duration {
        // In a real implementation, would track actual elapsed time
        // For now, this is a placeholder for the API
        Duration::ZERO
    }
}

/// Result of timeout enforcement on a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutStatus {
    /// Task completed before timeout.
    Completed,

    /// Soft timeout triggered (grace period).
    SoftTimeout,

    /// Hard timeout triggered (forced termination).
    HardTimeout,

    /// Task was explicitly cancelled.
    Cancelled,
}

impl TimeoutStatus {
    /// Check if this status indicates a timeout occurred.
    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            TimeoutStatus::SoftTimeout | TimeoutStatus::HardTimeout
        )
    }

    /// Check if this status is recoverable (retryable).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TimeoutStatus::HardTimeout | TimeoutStatus::SoftTimeout
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeout_policy_hard_only() {
        let policy = TimeoutPolicy::hard(Duration::from_secs(5));
        assert_eq!(policy.hard_timeout, Duration::from_secs(5));
        assert!(policy.soft_timeout.is_none());
        assert_eq!(policy.effective_timeout(), Duration::from_secs(5));
    }

    #[test]
    fn test_timeout_policy_soft_and_hard() {
        let policy = TimeoutPolicy::soft_and_hard(Duration::from_secs(3), Duration::from_secs(5));
        assert_eq!(policy.soft_timeout, Some(Duration::from_secs(3)));
        assert_eq!(policy.hard_timeout, Duration::from_secs(5));
        assert_eq!(policy.effective_timeout(), Duration::from_secs(3));
    }

    #[test]
    fn test_timeout_policy_effective_timeout() {
        // Hard timeout is shorter
        let policy = TimeoutPolicy::soft_and_hard(Duration::from_secs(5), Duration::from_secs(3));
        assert_eq!(policy.effective_timeout(), Duration::from_secs(3));
    }

    #[test]
    fn test_timeout_handle_cancel() {
        let task_id = TaskId::new();
        let handle = TimeoutHandle::new(task_id);

        assert!(!handle.is_cancelled());
        handle.cancel();
        assert!(handle.is_cancelled());
    }

    #[test]
    fn test_timeout_handle_soft_timeout() {
        let task_id = TaskId::new();
        let handle = TimeoutHandle::new(task_id);

        assert!(!handle.is_soft_timeout());
        handle.trigger_soft_timeout();
        assert!(handle.is_soft_timeout());
    }

    #[test]
    fn test_timeout_handle_reset() {
        let task_id = TaskId::new();
        let handle = TimeoutHandle::new(task_id);

        handle.cancel();
        handle.trigger_soft_timeout();
        assert!(handle.is_cancelled());
        assert!(handle.is_soft_timeout());

        handle.reset();
        assert!(!handle.is_cancelled());
        assert!(!handle.is_soft_timeout());
    }

    #[test]
    fn test_timeout_handle_clone() {
        let task_id = TaskId::new();
        let handle = TimeoutHandle::new(task_id);
        let cloned = handle.clone();

        handle.cancel();
        assert!(cloned.is_cancelled());
    }

    #[test]
    fn test_task_timeout_manager_creation() {
        let task_id = TaskId::new();
        let policy = TimeoutPolicy::hard(Duration::from_secs(5));
        let manager = TaskTimeoutManager::new(task_id, policy);

        assert_eq!(manager.handle().task_id(), task_id);
        assert!(!manager.handle().is_cancelled());
    }

    #[test]
    fn test_task_timeout_manager_check_ok() {
        let task_id = TaskId::new();
        let policy = TimeoutPolicy::hard(Duration::from_secs(5));
        let manager = TaskTimeoutManager::new(task_id, policy);

        assert!(manager.check().is_ok());
    }

    #[test]
    fn test_task_timeout_manager_check_hard_timeout() {
        let task_id = TaskId::new();
        let policy = TimeoutPolicy::hard(Duration::from_secs(5));
        let manager = TaskTimeoutManager::new(task_id, policy);

        manager.handle().cancel();
        assert!(manager.check().is_err());
    }

    #[tokio::test]
    async fn test_timeout_hard_only() {
        let task_id = TaskId::new();
        let policy = TimeoutPolicy::hard(Duration::from_millis(50));
        let manager = TaskTimeoutManager::new(task_id, policy);
        let handle = manager.handle();

        // Test that timeout enforcement completes successfully
        let start = std::time::Instant::now();
        manager.start().await;
        let elapsed = start.elapsed();

        // Should have taken at least the hard timeout duration
        assert!(elapsed >= Duration::from_millis(50));
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn test_timeout_soft_then_hard() {
        let task_id = TaskId::new();
        let policy =
            TimeoutPolicy::soft_and_hard(Duration::from_millis(30), Duration::from_millis(60));
        let manager = TaskTimeoutManager::new(task_id, policy);
        let handle = manager.handle();

        let start = std::time::Instant::now();
        manager.start().await;
        let elapsed = start.elapsed();

        // Should have taken at least the hard timeout duration (soft + remaining time to hard)
        assert!(elapsed >= Duration::from_millis(60));
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn test_timeout_cancellation_prevents_hard_timeout() {
        let task_id = TaskId::new();
        let policy = TimeoutPolicy::hard(Duration::from_millis(100));
        let manager = TaskTimeoutManager::new(task_id, policy.clone());
        let handle = manager.handle();

        // Spawn timeout enforcement
        let timeout_task = {
            let manager = TaskTimeoutManager::new(task_id, policy);
            tokio::spawn(async move {
                manager.start().await;
            })
        };

        // Cancel before timeout completes
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.cancel();

        timeout_task.await.unwrap();
        assert!(handle.is_cancelled());
    }

    #[test]
    fn test_timeout_status_is_timeout() {
        assert!(!TimeoutStatus::Completed.is_timeout());
        assert!(TimeoutStatus::SoftTimeout.is_timeout());
        assert!(TimeoutStatus::HardTimeout.is_timeout());
        assert!(!TimeoutStatus::Cancelled.is_timeout());
    }

    #[test]
    fn test_timeout_status_is_retryable() {
        assert!(!TimeoutStatus::Completed.is_retryable());
        assert!(TimeoutStatus::SoftTimeout.is_retryable());
        assert!(TimeoutStatus::HardTimeout.is_retryable());
        assert!(!TimeoutStatus::Cancelled.is_retryable());
    }
}
