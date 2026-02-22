//! Integration layer between task execution and queue backpressure management.
//!
//! This module provides `TaskExecutionFilter` for making task execution decisions
//! based on the current degradation mode. It enables:
//!
//! - **Graceful degradation**: Non-critical tasks rejected in Critical mode
//! - **Timeout adjustment**: Critical tasks get extended timeouts under load
//! - **Queue depth monitoring**: Real-time capacity reporting
//!
//! ## Usage
//!
//! ```ignore
//! let filter = TaskExecutionFilter::new(backpressure_manager);
//!
//! // Gate-keep before task execution
//! if filter.can_execute_task(&task).is_err() {
//!     // Reject non-critical task; optionally requeue with delay
//!     return;
//! }
//!
//! // Adjust timeout based on degradation mode
//! let timeout = base_duration.as_secs_f64() * filter.timeout_multiplier_for_task(&task);
//! ```

use crate::queue_backpressure::{DegradationMode, QueueBackpressureManager};
use crate::traits::TaskDescriptor;

/// Reasons a task may be rejected by the execution filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRejectionReason {
    /// The system is in Critical degradation mode and the task is not critical.
    DegradationModeRejectsNonCritical,
}

impl std::fmt::Display for TaskRejectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DegradationModeRejectsNonCritical => {
                write!(
                    f,
                    "non-critical task rejected: system in critical degradation mode"
                )
            }
        }
    }
}

/// Filters task execution decisions based on queue backpressure state.
///
/// This filter bridges task metadata (criticality) with system state (degradation mode)
/// to implement graceful degradation under load.
#[derive(Clone)]
pub struct TaskExecutionFilter {
    backpressure_manager: QueueBackpressureManager,
}

impl TaskExecutionFilter {
    /// Create a new filter with the given backpressure manager.
    pub fn new(manager: QueueBackpressureManager) -> Self {
        Self {
            backpressure_manager: manager,
        }
    }

    /// Check if a task can execute given the current degradation mode.
    ///
    /// Returns `Ok(())` if the task can execute, or an error reason if it should be rejected.
    ///
    /// Rules:
    /// - In Critical mode: only critical tasks are allowed
    /// - In all other modes: all tasks are allowed
    pub fn can_execute_task(&self, task: &TaskDescriptor) -> Result<(), TaskRejectionReason> {
        let mode = self.backpressure_manager.degradation_mode();

        // In Critical mode, reject non-critical tasks
        if mode == DegradationMode::Critical && !task.is_critical {
            return Err(TaskRejectionReason::DegradationModeRejectsNonCritical);
        }

        Ok(())
    }

    /// Get the timeout multiplier for a task based on its criticality and degradation mode.
    ///
    /// - Non-critical tasks always get 1.0x (base timeout)
    /// - Critical tasks get the degradation mode's timeout multiplier:
    ///   - Normal: 1.0x
    ///   - Moderate: 1.5x
    ///   - High: 2.5x
    ///   - Critical: 5.0x
    pub fn timeout_multiplier_for_task(&self, task: &TaskDescriptor) -> f64 {
        if task.is_critical {
            self.backpressure_manager
                .degradation_mode()
                .timeout_multiplier()
        } else {
            1.0
        }
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.backpressure_manager.queue_depth()
    }

    /// Get the current degradation mode.
    pub fn degradation_mode(&self) -> DegradationMode {
        self.backpressure_manager.degradation_mode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue_backpressure::BackpressureConfig;

    /// Helper to create a test task with default values.
    fn make_test_task(is_critical: bool) -> TaskDescriptor {
        TaskDescriptor {
            task_id: crate::ids::TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            resources: crate::resource::ResourceAllocation::default(),
            policy: crate::retry::RetryPolicy::default(),
            attempt: 0,
            is_critical,
        }
    }

    // ===== Task Rejection Tests =====

    #[test]
    fn test_non_critical_rejected_in_critical_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            critical_threshold: 0.95,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        // Fill queue to critical level (95+ items)
        for _ in 0..95 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Critical);

        // Non-critical task should be rejected
        let non_critical = make_test_task(false);
        let filter = TaskExecutionFilter::new(manager);
        assert!(matches!(
            filter.can_execute_task(&non_critical),
            Err(TaskRejectionReason::DegradationModeRejectsNonCritical)
        ));
    }

    #[test]
    fn test_critical_allowed_in_critical_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            critical_threshold: 0.95,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        // Fill queue to critical level
        for _ in 0..95 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Critical);

        // Critical task should be allowed
        let critical = make_test_task(true);
        let filter = TaskExecutionFilter::new(manager);
        assert!(filter.can_execute_task(&critical).is_ok());
    }

    #[test]
    fn test_non_critical_allowed_in_normal_mode() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());
        assert_eq!(manager.degradation_mode(), DegradationMode::Normal);

        let non_critical = make_test_task(false);
        let filter = TaskExecutionFilter::new(manager);
        assert!(filter.can_execute_task(&non_critical).is_ok());
    }

    #[test]
    fn test_non_critical_allowed_in_moderate_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            moderate_threshold: 0.5,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        // Fill queue to moderate level (50-79 items)
        for _ in 0..60 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Moderate);

        let non_critical = make_test_task(false);
        let filter = TaskExecutionFilter::new(manager);
        assert!(filter.can_execute_task(&non_critical).is_ok());
    }

    #[test]
    fn test_non_critical_allowed_in_high_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            high_threshold: 0.8,
            critical_threshold: 0.95,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        // Fill queue to high level (80-94 items)
        for _ in 0..85 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::High);

        let non_critical = make_test_task(false);
        let filter = TaskExecutionFilter::new(manager);
        assert!(filter.can_execute_task(&non_critical).is_ok());
    }

    // ===== Degradation Mode Transition Tests =====

    #[test]
    fn test_transition_to_moderate_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            moderate_threshold: 0.5,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);
        assert_eq!(manager.degradation_mode(), DegradationMode::Normal);

        // Fill to moderate level
        for _ in 0..50 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Moderate);

        // Tasks still allowed
        let filter = TaskExecutionFilter::new(manager.clone());
        assert!(filter.can_execute_task(&make_test_task(false)).is_ok());
    }

    #[test]
    fn test_transition_to_high_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            high_threshold: 0.8,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        for _ in 0..80 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::High);

        let filter = TaskExecutionFilter::new(manager);
        assert!(filter.can_execute_task(&make_test_task(false)).is_ok());
    }

    #[test]
    fn test_transition_to_critical_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            critical_threshold: 0.95,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        for _ in 0..95 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Critical);

        let filter = TaskExecutionFilter::new(manager);
        assert!(filter.can_execute_task(&make_test_task(false)).is_err());
    }

    #[test]
    fn test_transition_back_to_normal() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            moderate_threshold: 0.5,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        // Fill to moderate
        for _ in 0..60 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Moderate);

        // Drain below moderate
        for _ in 0..20 {
            manager.dequeue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Normal);

        let filter = TaskExecutionFilter::new(manager);
        assert!(filter.can_execute_task(&make_test_task(false)).is_ok());
    }

    // ===== Timeout Adjustment Tests =====

    #[test]
    fn test_timeout_multiplier_non_critical_in_normal_mode() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());
        assert_eq!(manager.degradation_mode(), DegradationMode::Normal);

        let filter = TaskExecutionFilter::new(manager);
        let task = make_test_task(false);
        assert_eq!(filter.timeout_multiplier_for_task(&task), 1.0);
    }

    #[test]
    fn test_timeout_multiplier_non_critical_in_critical_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            critical_threshold: 0.95,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        for _ in 0..95 {
            manager.enqueue();
        }

        let filter = TaskExecutionFilter::new(manager);
        let task = make_test_task(false);
        // Even in critical mode, non-critical tasks get 1.0x (if they were allowed, which they're not)
        assert_eq!(filter.timeout_multiplier_for_task(&task), 1.0);
    }

    #[test]
    fn test_timeout_multiplier_critical_in_normal_mode() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());
        assert_eq!(manager.degradation_mode(), DegradationMode::Normal);

        let filter = TaskExecutionFilter::new(manager);
        let task = make_test_task(true);
        assert_eq!(filter.timeout_multiplier_for_task(&task), 1.0);
    }

    #[test]
    fn test_timeout_multiplier_critical_in_moderate_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            moderate_threshold: 0.5,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        for _ in 0..50 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Moderate);

        let filter = TaskExecutionFilter::new(manager);
        let task = make_test_task(true);
        assert_eq!(filter.timeout_multiplier_for_task(&task), 1.5);
    }

    #[test]
    fn test_timeout_multiplier_critical_in_high_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            high_threshold: 0.8,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        for _ in 0..80 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::High);

        let filter = TaskExecutionFilter::new(manager);
        let task = make_test_task(true);
        assert_eq!(filter.timeout_multiplier_for_task(&task), 2.5);
    }

    #[test]
    fn test_timeout_multiplier_critical_in_critical_mode() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            critical_threshold: 0.95,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        for _ in 0..95 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Critical);

        let filter = TaskExecutionFilter::new(manager);
        let task = make_test_task(true);
        assert_eq!(filter.timeout_multiplier_for_task(&task), 5.0);
    }

    #[test]
    fn test_timeout_multiplier_all_degradation_modes() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            moderate_threshold: 0.5,
            high_threshold: 0.8,
            critical_threshold: 0.95,
        };

        // Test each degradation mode
        let test_cases = vec![
            (0, DegradationMode::Normal, 1.0),
            (50, DegradationMode::Moderate, 1.5),
            (80, DegradationMode::High, 2.5),
            (95, DegradationMode::Critical, 5.0),
        ];

        for (fill_count, expected_mode, expected_multiplier) in test_cases {
            let manager = QueueBackpressureManager::new(config.clone());

            for _ in 0..fill_count {
                manager.enqueue();
            }

            assert_eq!(manager.degradation_mode(), expected_mode);

            let filter = TaskExecutionFilter::new(manager);
            let task = make_test_task(true);
            assert_eq!(
                filter.timeout_multiplier_for_task(&task),
                expected_multiplier,
                "Expected {} for mode {:?}",
                expected_multiplier,
                expected_mode
            );
        }
    }

    // ===== Queue Depth Tracking Tests =====

    #[test]
    fn test_queue_depth_reflects_backpressure_manager_state() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());
        let filter = TaskExecutionFilter::new(manager.clone());

        assert_eq!(filter.queue_depth(), 0);

        for i in 1..=50 {
            manager.enqueue();
            assert_eq!(filter.queue_depth(), i);
        }

        for i in (1..=50).rev() {
            manager.dequeue();
            assert_eq!(filter.queue_depth(), i - 1);
        }
    }

    #[test]
    fn test_queue_depth_zero_on_empty() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());
        let filter = TaskExecutionFilter::new(manager);

        assert_eq!(filter.queue_depth(), 0);
        assert_eq!(filter.degradation_mode(), DegradationMode::Normal);
    }

    #[test]
    fn test_degradation_mode_getter() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            moderate_threshold: 0.5,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);
        let filter = TaskExecutionFilter::new(manager.clone());

        assert_eq!(filter.degradation_mode(), DegradationMode::Normal);

        for _ in 0..50 {
            manager.enqueue();
        }

        assert_eq!(filter.degradation_mode(), DegradationMode::Moderate);
    }

    // ===== Error Message Tests =====

    #[test]
    fn test_rejection_reason_display() {
        let reason = TaskRejectionReason::DegradationModeRejectsNonCritical;
        let msg = format!("{}", reason);
        assert!(msg.contains("non-critical task rejected"));
        assert!(msg.contains("critical degradation mode"));
    }

    // ===== Clone Tests =====

    #[test]
    fn test_filter_clone_shares_state() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());
        let filter1 = TaskExecutionFilter::new(manager.clone());

        manager.enqueue();
        manager.enqueue();

        let filter2 = filter1.clone();
        assert_eq!(filter1.queue_depth(), 2);
        assert_eq!(filter2.queue_depth(), 2);
    }
}
