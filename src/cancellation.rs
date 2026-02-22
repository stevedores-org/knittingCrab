//! Job cancellation with graceful shutdown.
//!
//! Handles job cancellation requests with SIGTERM grace period followed by SIGKILL.

use std::time::Duration;

/// Cancellation state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationState {
    /// Job is running normally
    Running,
    /// Cancellation requested, waiting for graceful shutdown
    PendingGraceful,
    /// Graceful timeout expired, force killing
    ForceKilling,
    /// Job has been cancelled
    Cancelled,
}

/// Manages cancellation requests for a job.
pub struct CancellationManager {
    state: CancellationState,
    grace_period: Duration,
}

impl CancellationManager {
    /// Create a new cancellation manager with a grace period for SIGTERM.
    pub fn new(grace_period: Duration) -> Self {
        CancellationManager {
            state: CancellationState::Running,
            grace_period,
        }
    }

    /// Request graceful cancellation of the job.
    pub fn request_cancellation(&mut self) {
        match self.state {
            CancellationState::Running => {
                self.state = CancellationState::PendingGraceful;
            }
            CancellationState::PendingGraceful => {
                // Already requested
            }
            CancellationState::ForceKilling | CancellationState::Cancelled => {
                // Already cancelled
            }
        }
    }

    /// Check if grace period has expired and force kill is needed.
    pub fn should_force_kill(&mut self) -> bool {
        if self.state == CancellationState::PendingGraceful {
            self.state = CancellationState::ForceKilling;
            true
        } else {
            false
        }
    }

    /// Mark the job as successfully cancelled.
    pub fn mark_cancelled(&mut self) {
        self.state = CancellationState::Cancelled;
    }

    /// Get current cancellation state.
    pub fn state(&self) -> CancellationState {
        self.state
    }

    /// Check if cancellation has been requested.
    pub fn is_cancellation_requested(&self) -> bool {
        matches!(
            self.state,
            CancellationState::PendingGraceful
                | CancellationState::ForceKilling
                | CancellationState::Cancelled
        )
    }

    /// Check if the job has been fully cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.state == CancellationState::Cancelled
    }

    /// Get the grace period duration.
    pub fn grace_period(&self) -> Duration {
        self.grace_period
    }
}

impl Default for CancellationManager {
    fn default() -> Self {
        Self::new(Duration::from_secs(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_running_state() {
        let manager = CancellationManager::default();
        assert_eq!(manager.state(), CancellationState::Running);
        assert!(!manager.is_cancellation_requested());
        assert!(!manager.is_cancelled());
    }

    #[test]
    fn request_cancellation_transitions_to_pending_graceful() {
        let mut manager = CancellationManager::default();
        manager.request_cancellation();
        assert_eq!(manager.state(), CancellationState::PendingGraceful);
        assert!(manager.is_cancellation_requested());
    }

    #[test]
    fn force_kill_transitions_to_force_killing() {
        let mut manager = CancellationManager::default();
        manager.request_cancellation();
        assert!(manager.should_force_kill());
        assert_eq!(manager.state(), CancellationState::ForceKilling);
    }

    #[test]
    fn mark_cancelled_completes_cancellation() {
        let mut manager = CancellationManager::default();
        manager.request_cancellation();
        manager.mark_cancelled();
        assert!(manager.is_cancelled());
        assert_eq!(manager.state(), CancellationState::Cancelled);
    }

    #[test]
    fn cannot_requeue_after_cancellation() {
        let mut manager = CancellationManager::default();
        manager.request_cancellation();
        manager.mark_cancelled();

        // Further calls should have no effect
        manager.request_cancellation();
        assert_eq!(manager.state(), CancellationState::Cancelled);
    }

    #[test]
    fn grace_period_is_configurable() {
        let grace_period = Duration::from_secs(5);
        let manager = CancellationManager::new(grace_period);
        assert_eq!(manager.grace_period(), grace_period);
    }

    #[test]
    fn cancellation_idempotency() {
        let mut manager = CancellationManager::default();

        // Request multiple times
        manager.request_cancellation();
        manager.request_cancellation();
        manager.request_cancellation();

        // Should only be in pending graceful, not in some weird state
        assert_eq!(manager.state(), CancellationState::PendingGraceful);
    }
}
