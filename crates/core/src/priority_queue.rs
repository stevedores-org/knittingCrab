//! Priority-aware task queue manager with degradation awareness.
//!
//! This module implements a 4-queue system that routes tasks based on priority
//! and respects system degradation modes to implement graceful degradation.
//!
//! ## Architecture
//!
//! The queue maintains 4 separate internal VecDeques:
//! - Critical queue: System-critical work (always accessible)
//! - High queue: Important work with SLAs
//! - Normal queue: General user work
//! - Low queue: Background/deferred work
//!
//! ## Degradation Interaction
//!
//! When the system enters different degradation modes, queue access is restricted:
//! - Normal mode: All queues accessible
//! - Moderate mode: All queues accessible (info for scheduler)
//! - High mode: Skip Low tasks (via scheduler)
//! - Critical mode: Only Critical queue accessible

use crate::priority::Priority;
use crate::queue_backpressure::DegradationMode;
use crate::traits::TaskDescriptor;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Statistics snapshot of priority queue state.
#[derive(Debug, Clone)]
pub struct PriorityQueueStats {
    /// Number of tasks in Critical queue.
    pub critical_count: usize,
    /// Number of tasks in High queue.
    pub high_count: usize,
    /// Number of tasks in Normal queue.
    pub normal_count: usize,
    /// Number of tasks in Low queue.
    pub low_count: usize,
    /// Total across all queues.
    pub total_count: usize,
    /// Highest priority level with pending tasks (None if empty).
    pub highest_priority: Option<Priority>,
}

/// Priority-aware task queue manager.
///
/// Maintains 4 separate queues (one per priority level) and routes tasks
/// based on their priority. Respects system degradation modes when dequeuing.
pub struct PriorityQueueManager {
    critical_queue: Arc<Mutex<VecDeque<TaskDescriptor>>>,
    high_queue: Arc<Mutex<VecDeque<TaskDescriptor>>>,
    normal_queue: Arc<Mutex<VecDeque<TaskDescriptor>>>,
    low_queue: Arc<Mutex<VecDeque<TaskDescriptor>>>,
    total_depth: Arc<AtomicUsize>,
}

impl PriorityQueueManager {
    /// Create a new priority queue manager.
    pub fn new() -> Self {
        Self {
            critical_queue: Arc::new(Mutex::new(VecDeque::new())),
            high_queue: Arc::new(Mutex::new(VecDeque::new())),
            normal_queue: Arc::new(Mutex::new(VecDeque::new())),
            low_queue: Arc::new(Mutex::new(VecDeque::new())),
            total_depth: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Enqueue a task in the queue corresponding to its priority.
    ///
    /// # Example
    /// ```ignore
    /// let task = TaskDescriptor { priority: Priority::High, ... };
    /// manager.enqueue_task(task)?;
    /// ```
    pub fn enqueue_task(&self, task: TaskDescriptor) -> Result<(), crate::error::CoreError> {
        let priority = task.priority;

        let queue = match priority {
            Priority::Critical => &self.critical_queue,
            Priority::High => &self.high_queue,
            Priority::Normal => &self.normal_queue,
            Priority::Low => &self.low_queue,
        };

        let mut q = queue.lock().map_err(|e| {
            crate::error::CoreError::Internal(format!("queue lock poisoned: {}", e))
        })?;

        q.push_back(task);
        self.total_depth.fetch_add(1, Ordering::SeqCst);

        Ok(())
    }

    /// Dequeue the next task respecting priority order and degradation mode.
    ///
    /// Dequeuing order respects degradation mode:
    /// - Normal/Moderate: Try Critical, High, Normal, Low (in order)
    /// - High: Try Critical, High, Normal (skip Low)
    /// - Critical: Only Critical queue
    ///
    /// Returns the first non-empty queue found.
    pub fn dequeue_task(
        &self,
        degradation_mode: DegradationMode,
    ) -> Result<Option<TaskDescriptor>, crate::error::CoreError> {
        // Determine which queues to check based on degradation mode
        let queues_to_check: Vec<(Priority, &Arc<Mutex<VecDeque<TaskDescriptor>>>)> =
            match degradation_mode {
                DegradationMode::Normal | DegradationMode::Moderate => {
                    vec![
                        (Priority::Critical, &self.critical_queue),
                        (Priority::High, &self.high_queue),
                        (Priority::Normal, &self.normal_queue),
                        (Priority::Low, &self.low_queue),
                    ]
                }
                DegradationMode::High => {
                    vec![
                        (Priority::Critical, &self.critical_queue),
                        (Priority::High, &self.high_queue),
                        (Priority::Normal, &self.normal_queue),
                    ]
                }
                DegradationMode::Critical => {
                    vec![(Priority::Critical, &self.critical_queue)]
                }
            };

        // Try to dequeue from each queue in order
        for (_priority, queue) in queues_to_check {
            let mut q = queue.lock().map_err(|e| {
                crate::error::CoreError::Internal(format!("queue lock poisoned: {}", e))
            })?;

            if let Some(task) = q.pop_front() {
                self.total_depth.fetch_sub(1, Ordering::SeqCst);
                return Ok(Some(task));
            }
        }

        Ok(None)
    }

    /// Get the number of pending tasks at a specific priority level.
    pub fn pending_at_priority(
        &self,
        priority: Priority,
    ) -> Result<usize, crate::error::CoreError> {
        let queue = match priority {
            Priority::Critical => &self.critical_queue,
            Priority::High => &self.high_queue,
            Priority::Normal => &self.normal_queue,
            Priority::Low => &self.low_queue,
        };

        let q = queue.lock().map_err(|e| {
            crate::error::CoreError::Internal(format!("queue lock poisoned: {}", e))
        })?;

        Ok(q.len())
    }

    /// Get the total number of pending tasks across all queues.
    pub fn total_depth(&self) -> usize {
        self.total_depth.load(Ordering::SeqCst)
    }

    /// Get statistics snapshot of all queues.
    pub fn stats(&self) -> Result<PriorityQueueStats, crate::error::CoreError> {
        let critical = self.pending_at_priority(Priority::Critical)?;
        let high = self.pending_at_priority(Priority::High)?;
        let normal = self.pending_at_priority(Priority::Normal)?;
        let low = self.pending_at_priority(Priority::Low)?;

        let total = critical + high + normal + low;

        let highest_priority = if critical > 0 {
            Some(Priority::Critical)
        } else if high > 0 {
            Some(Priority::High)
        } else if normal > 0 {
            Some(Priority::Normal)
        } else if low > 0 {
            Some(Priority::Low)
        } else {
            None
        };

        Ok(PriorityQueueStats {
            critical_count: critical,
            high_count: high,
            normal_count: normal,
            low_count: low,
            total_count: total,
            highest_priority,
        })
    }

    /// Clear all queues (primarily for testing).
    pub fn clear(&self) -> Result<(), crate::error::CoreError> {
        let mut c = self.critical_queue.lock().map_err(|e| {
            crate::error::CoreError::Internal(format!("queue lock poisoned: {}", e))
        })?;
        let mut h = self.high_queue.lock().map_err(|e| {
            crate::error::CoreError::Internal(format!("queue lock poisoned: {}", e))
        })?;
        let mut n = self.normal_queue.lock().map_err(|e| {
            crate::error::CoreError::Internal(format!("queue lock poisoned: {}", e))
        })?;
        let mut l = self.low_queue.lock().map_err(|e| {
            crate::error::CoreError::Internal(format!("queue lock poisoned: {}", e))
        })?;

        c.clear();
        h.clear();
        n.clear();
        l.clear();

        self.total_depth.store(0, Ordering::SeqCst);

        Ok(())
    }
}

impl Clone for PriorityQueueManager {
    fn clone(&self) -> Self {
        Self {
            critical_queue: Arc::clone(&self.critical_queue),
            high_queue: Arc::clone(&self.high_queue),
            normal_queue: Arc::clone(&self.normal_queue),
            low_queue: Arc::clone(&self.low_queue),
            total_depth: Arc::clone(&self.total_depth),
        }
    }
}

impl Default for PriorityQueueManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TaskId;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_task(task_id: TaskId, priority: Priority) -> TaskDescriptor {
        TaskDescriptor {
            task_id,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: crate::resource::ResourceAllocation::default(),
            policy: crate::retry::RetryPolicy::default(),
            attempt: 0,
            is_critical: priority.is_critical(),
            priority,
            goal: None,
            budget: None,
            test_gate: None,
        }
    }

    #[test]
    fn test_enqueue_routes_to_correct_queue() {
        let manager = PriorityQueueManager::new();

        let task_critical = make_task(TaskId::new(), Priority::Critical);
        let task_high = make_task(TaskId::new(), Priority::High);
        let task_normal = make_task(TaskId::new(), Priority::Normal);
        let task_low = make_task(TaskId::new(), Priority::Low);

        assert!(manager.enqueue_task(task_critical).is_ok());
        assert!(manager.enqueue_task(task_high).is_ok());
        assert!(manager.enqueue_task(task_normal).is_ok());
        assert!(manager.enqueue_task(task_low).is_ok());

        let stats = manager.stats().unwrap();
        assert_eq!(stats.critical_count, 1);
        assert_eq!(stats.high_count, 1);
        assert_eq!(stats.normal_count, 1);
        assert_eq!(stats.low_count, 1);
        assert_eq!(stats.total_count, 4);
    }

    #[test]
    fn test_dequeue_respects_priority_order() {
        let manager = PriorityQueueManager::new();

        let task_low = make_task(TaskId::new(), Priority::Low);
        let task_high = make_task(TaskId::new(), Priority::High);
        let task_critical = make_task(TaskId::new(), Priority::Critical);

        // Enqueue in reverse priority order
        manager.enqueue_task(task_low.clone()).unwrap();
        manager.enqueue_task(task_high.clone()).unwrap();
        manager.enqueue_task(task_critical.clone()).unwrap();

        // Should dequeue in priority order (Critical, High, Low)
        let dequeued1 = manager
            .dequeue_task(DegradationMode::Normal)
            .unwrap()
            .unwrap();
        assert_eq!(dequeued1.task_id, task_critical.task_id);

        let dequeued2 = manager
            .dequeue_task(DegradationMode::Normal)
            .unwrap()
            .unwrap();
        assert_eq!(dequeued2.task_id, task_high.task_id);

        let dequeued3 = manager
            .dequeue_task(DegradationMode::Normal)
            .unwrap()
            .unwrap();
        assert_eq!(dequeued3.task_id, task_low.task_id);
    }

    #[test]
    fn test_dequeue_fifo_within_priority() {
        let manager = PriorityQueueManager::new();

        let task1 = make_task(TaskId::new(), Priority::High);
        let task2 = make_task(TaskId::new(), Priority::High);
        let task3 = make_task(TaskId::new(), Priority::High);

        let id1 = task1.task_id;
        let id2 = task2.task_id;
        let id3 = task3.task_id;

        manager.enqueue_task(task1).unwrap();
        manager.enqueue_task(task2).unwrap();
        manager.enqueue_task(task3).unwrap();

        assert_eq!(
            manager
                .dequeue_task(DegradationMode::Normal)
                .unwrap()
                .unwrap()
                .task_id,
            id1
        );
        assert_eq!(
            manager
                .dequeue_task(DegradationMode::Normal)
                .unwrap()
                .unwrap()
                .task_id,
            id2
        );
        assert_eq!(
            manager
                .dequeue_task(DegradationMode::Normal)
                .unwrap()
                .unwrap()
                .task_id,
            id3
        );
    }

    #[test]
    fn test_dequeue_empty_returns_none() {
        let manager = PriorityQueueManager::new();

        let result = manager.dequeue_task(DegradationMode::Normal).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_dequeue_with_degradation_mode_critical() {
        let manager = PriorityQueueManager::new();

        let task_critical = make_task(TaskId::new(), Priority::Critical);
        let task_high = make_task(TaskId::new(), Priority::High);
        let task_normal = make_task(TaskId::new(), Priority::Normal);

        manager.enqueue_task(task_critical.clone()).unwrap();
        manager.enqueue_task(task_high).unwrap();
        manager.enqueue_task(task_normal).unwrap();

        // In Critical degradation mode, only Critical queue is accessible
        let dequeued = manager
            .dequeue_task(DegradationMode::Critical)
            .unwrap()
            .unwrap();
        assert_eq!(dequeued.task_id, task_critical.task_id);

        // High and Normal should still be queued, not accessible in Critical mode
        assert!(manager
            .dequeue_task(DegradationMode::Critical)
            .unwrap()
            .is_none());

        // In Normal mode, all are accessible
        let dequeued = manager
            .dequeue_task(DegradationMode::Normal)
            .unwrap()
            .unwrap();
        assert_eq!(dequeued.priority, Priority::High);
    }

    #[test]
    fn test_dequeue_with_degradation_mode_high() {
        let manager = PriorityQueueManager::new();

        let task_high = make_task(TaskId::new(), Priority::High);
        let task_low = make_task(TaskId::new(), Priority::Low);

        manager.enqueue_task(task_high.clone()).unwrap();
        manager.enqueue_task(task_low).unwrap();

        // In High degradation mode, Low queue is skipped
        let dequeued = manager
            .dequeue_task(DegradationMode::High)
            .unwrap()
            .unwrap();
        assert_eq!(dequeued.priority, Priority::High);

        // Low is still there but not accessible in High mode
        assert!(manager
            .dequeue_task(DegradationMode::High)
            .unwrap()
            .is_none());

        // In Normal mode, Low is accessible
        let dequeued = manager
            .dequeue_task(DegradationMode::Normal)
            .unwrap()
            .unwrap();
        assert_eq!(dequeued.priority, Priority::Low);
    }

    #[test]
    fn test_pending_at_priority() {
        let manager = PriorityQueueManager::new();

        for _ in 0..3 {
            manager
                .enqueue_task(make_task(TaskId::new(), Priority::Critical))
                .unwrap();
        }
        for _ in 0..2 {
            manager
                .enqueue_task(make_task(TaskId::new(), Priority::High))
                .unwrap();
        }

        assert_eq!(manager.pending_at_priority(Priority::Critical).unwrap(), 3);
        assert_eq!(manager.pending_at_priority(Priority::High).unwrap(), 2);
        assert_eq!(manager.pending_at_priority(Priority::Normal).unwrap(), 0);
        assert_eq!(manager.pending_at_priority(Priority::Low).unwrap(), 0);
    }

    #[test]
    fn test_total_depth() {
        let manager = PriorityQueueManager::new();

        assert_eq!(manager.total_depth(), 0);

        for _ in 0..5 {
            manager
                .enqueue_task(make_task(TaskId::new(), Priority::High))
                .unwrap();
        }
        assert_eq!(manager.total_depth(), 5);

        manager
            .dequeue_task(DegradationMode::Normal)
            .unwrap()
            .unwrap();
        assert_eq!(manager.total_depth(), 4);
    }

    #[test]
    fn test_stats_highest_priority() {
        let manager = PriorityQueueManager::new();

        assert_eq!(manager.stats().unwrap().highest_priority, None);

        manager
            .enqueue_task(make_task(TaskId::new(), Priority::Low))
            .unwrap();
        assert_eq!(
            manager.stats().unwrap().highest_priority,
            Some(Priority::Low)
        );

        manager
            .enqueue_task(make_task(TaskId::new(), Priority::Critical))
            .unwrap();
        assert_eq!(
            manager.stats().unwrap().highest_priority,
            Some(Priority::Critical)
        );
    }

    #[test]
    fn test_clear() {
        let manager = PriorityQueueManager::new();

        for _ in 0..5 {
            manager
                .enqueue_task(make_task(TaskId::new(), Priority::High))
                .unwrap();
        }
        assert_eq!(manager.total_depth(), 5);

        manager.clear().unwrap();
        assert_eq!(manager.total_depth(), 0);
        assert!(manager
            .dequeue_task(DegradationMode::Normal)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_clone_shares_state() {
        let manager = PriorityQueueManager::new();

        manager
            .enqueue_task(make_task(TaskId::new(), Priority::High))
            .unwrap();
        manager
            .enqueue_task(make_task(TaskId::new(), Priority::Normal))
            .unwrap();

        let cloned = manager.clone();

        assert_eq!(manager.total_depth(), 2);
        assert_eq!(cloned.total_depth(), 2);

        cloned.dequeue_task(DegradationMode::Normal).unwrap();

        // Both should reflect the dequeue
        assert_eq!(manager.total_depth(), 1);
        assert_eq!(cloned.total_depth(), 1);
    }
}
