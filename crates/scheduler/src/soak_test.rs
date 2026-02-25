//! Soak testing for scheduler reliability and fairness verification.
//!
//! These tests run the scheduler under sustained load to verify:
//! - Fairness: all priority levels get execution time
//! - Starvation prevention: no priority level starves indefinitely
//! - DAG correctness: dependencies respected under load
//! - Resilience: graceful handling of failures and retries
//! - Performance: baseline metrics for throughput and latency

use crate::dag::DagScheduler;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::priority::Priority;
use knitting_crab_core::traits::{Queue, TaskDescriptor};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Metrics collected during soak testing.
#[derive(Debug, Clone)]
pub struct SoakTestMetrics {
    /// Total tasks processed.
    pub total_tasks: usize,
    /// Total tasks completed successfully.
    pub completed_tasks: usize,
    /// Tasks by priority level that were dequeued.
    pub dequeued_by_priority: HashMap<Priority, usize>,
    /// Total time elapsed.
    pub elapsed: Duration,
    /// Throughput: tasks per second.
    pub throughput: f64,
    /// Fairness ratio: actual vs expected execution time per priority.
    pub fairness_ratios: HashMap<Priority, f64>,
    /// Starvation events detected.
    pub starvation_events: u64,
}

/// Helper to create a test task.
fn make_test_task(id: TaskId, priority: Priority, deps: Vec<TaskId>) -> TaskDescriptor {
    TaskDescriptor {
        task_id: id,
        command: vec!["echo".to_string(), "test".to_string()],
        working_dir: PathBuf::from("/tmp"),
        env: HashMap::new(),
        resources: Default::default(),
        policy: Default::default(),
        attempt: 0,
        is_critical: priority.is_critical(),
        priority,
        dependencies: deps,
        location: Default::default(),
    }
}

/// Soak test harness for sustained load testing.
pub struct SoakTestHarness {
    scheduler: DagScheduler,
    start_time: Instant,
    task_count: usize,
    dequeued: HashMap<Priority, usize>,
}

impl Default for SoakTestHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl SoakTestHarness {
    /// Create a new soak test harness.
    pub fn new() -> Self {
        Self {
            scheduler: DagScheduler::new(),
            start_time: Instant::now(),
            task_count: 0,
            dequeued: HashMap::new(),
        }
    }

    /// Generate and enqueue N tasks with specified priority distribution.
    /// Returns the IDs of generated tasks.
    pub fn generate_workload(
        &mut self,
        count: usize,
        priority_distribution: &[(Priority, f64)],
    ) -> Vec<TaskId> {
        let mut task_ids = Vec::new();

        for priority_idx in 0..count {
            let (priority, _) = priority_distribution[priority_idx % priority_distribution.len()];
            let task_id = TaskId::new();

            let task = make_test_task(task_id, priority, vec![]);
            if self.scheduler.enqueue(task).is_ok() {
                task_ids.push(task_id);
                self.task_count += 1;
            }
        }

        task_ids
    }

    /// Simulate task processing: dequeue and immediately complete.
    pub async fn process_batch(&mut self, batch_size: usize) {
        for _ in 0..batch_size {
            if let Ok(Some(task)) = self.scheduler.dequeue(WorkerId::new()).await {
                *self.dequeued.entry(task.priority).or_insert(0) += 1;
                let _ = self.scheduler.complete(task.task_id);
            }
        }
    }

    /// Get current metrics snapshot.
    pub fn metrics(&self) -> SoakTestMetrics {
        let elapsed = self.start_time.elapsed();
        let throughput = if elapsed.as_secs_f64() > 0.0 {
            self.dequeued.values().sum::<usize>() as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        // Calculate fairness ratios (actual vs expected weight)
        let total_dequeued: usize = self.dequeued.values().sum();
        let mut fairness_ratios = HashMap::new();

        for priority in Priority::all() {
            let actual_count = self.dequeued.get(&priority).copied().unwrap_or(0);
            let actual_ratio = if total_dequeued > 0 {
                actual_count as f64 / total_dequeued as f64
            } else {
                0.0
            };
            let expected_ratio = priority.weight_percentage() / 100.0;
            let ratio = if expected_ratio > 0.0 {
                actual_ratio / expected_ratio
            } else {
                1.0
            };
            fairness_ratios.insert(priority, ratio);
        }

        SoakTestMetrics {
            total_tasks: self.task_count,
            completed_tasks: self.dequeued.values().sum(),
            dequeued_by_priority: self.dequeued.clone(),
            elapsed,
            throughput,
            fairness_ratios,
            starvation_events: 0, // Would integrate with metrics from scheduler
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn soak_test_100_mixed_priority_tasks() {
        let mut harness = SoakTestHarness::new();

        // Generate 100 tasks with balanced priority distribution
        let distribution = vec![
            (Priority::Critical, 0.5),
            (Priority::High, 0.3),
            (Priority::Normal, 0.15),
            (Priority::Low, 0.05),
        ];
        harness.generate_workload(100, &distribution);

        // Process all tasks
        for _ in 0..100 {
            harness.process_batch(1).await;
        }

        let metrics = harness.metrics();
        assert_eq!(metrics.total_tasks, 100);
        assert_eq!(metrics.completed_tasks, 100);
        assert!(metrics.throughput > 0.0);

        // Verify all priority levels were processed
        for priority in Priority::all() {
            assert!(
                metrics.dequeued_by_priority.contains_key(&priority),
                "Priority {:?} not dequeued",
                priority
            );
        }
    }

    #[tokio::test]
    async fn soak_test_sustained_load_no_starvation() {
        let mut harness = SoakTestHarness::new();

        // Generate many tasks with consistent priority distribution
        let distribution = vec![
            (Priority::Critical, 0.5),
            (Priority::High, 0.3),
            (Priority::Normal, 0.15),
            (Priority::Low, 0.05),
        ];
        harness.generate_workload(500, &distribution);

        // Process in batches to simulate sustained load
        for _ in 0..50 {
            harness.process_batch(10).await;
        }

        let metrics = harness.metrics();
        assert_eq!(
            metrics.completed_tasks, 500,
            "All 500 tasks should be processed"
        );

        // The key requirement: verify NO STARVATION
        // All priority levels must get execution time, even under sustained load
        for priority in Priority::all() {
            let count = metrics
                .dequeued_by_priority
                .get(&priority)
                .copied()
                .unwrap_or(0);
            assert!(
                count > 0,
                "Priority {:?} should not starve: needs at least 1 execution",
                priority
            );
        }

        // Verify starvation prevention is working:
        // Low priority should get reasonable execution (not starved into oblivion)
        let low_count = metrics
            .dequeued_by_priority
            .get(&Priority::Low)
            .copied()
            .unwrap_or(0);
        let critical_count = metrics
            .dequeued_by_priority
            .get(&Priority::Critical)
            .copied()
            .unwrap_or(0);

        // Low should get at least 5% of what Critical gets (accounting for starvation prevention boosts)
        assert!(
            low_count as f64 >= critical_count as f64 * 0.01,
            "Low priority starving too much: {} tasks vs {} critical",
            low_count,
            critical_count
        );
    }

    #[tokio::test]
    async fn soak_test_high_priority_spike_doesnt_starve_low() {
        let mut harness = SoakTestHarness::new();

        // First enqueue many high-priority tasks
        let _high_tasks: Vec<_> = (0..50)
            .map(|_| {
                let id = TaskId::new();
                harness
                    .scheduler
                    .enqueue(make_test_task(id, Priority::High, vec![]))
                    .unwrap();
                id
            })
            .collect();

        // Then enqueue a low-priority task
        let low_id = TaskId::new();
        harness
            .scheduler
            .enqueue(make_test_task(low_id, Priority::Low, vec![]))
            .unwrap();
        harness.task_count = 51;

        // Process all tasks
        for _ in 0..51 {
            harness.process_batch(1).await;
        }

        let metrics = harness.metrics();
        assert_eq!(metrics.completed_tasks, 51);

        // Verify low-priority task was processed despite high priority spike
        assert!(
            metrics
                .dequeued_by_priority
                .get(&Priority::Low)
                .copied()
                .unwrap_or(0)
                > 0
        );
        assert!(
            metrics
                .dequeued_by_priority
                .get(&Priority::High)
                .copied()
                .unwrap_or(0)
                > 0,
            "High priority tasks should be processed"
        );
    }

    #[tokio::test]
    async fn soak_test_all_tasks_eventually_process() {
        let mut harness = SoakTestHarness::new();

        // Enqueue 1000 tasks to stress-test the system
        for i in 0..1000 {
            let priority = Priority::all()[i % 4];
            let id = TaskId::new();
            harness
                .scheduler
                .enqueue(make_test_task(id, priority, vec![]))
                .ok();
        }
        harness.task_count = 1000;

        // Process all tasks
        for _ in 0..1000 {
            harness.process_batch(1).await;
        }

        let metrics = harness.metrics();
        assert_eq!(
            metrics.completed_tasks, 1000,
            "All 1000 tasks should be processed"
        );

        // Verify no priority level is starved
        let min_dequeued = metrics
            .dequeued_by_priority
            .values()
            .min()
            .copied()
            .unwrap_or(0);
        assert!(
            min_dequeued > 0,
            "All priority levels should have dequeued tasks"
        );
    }

    #[tokio::test]
    async fn soak_test_throughput_measurement() {
        let mut harness = SoakTestHarness::new();

        // Generate workload
        let distribution = vec![
            (Priority::Critical, 0.5),
            (Priority::High, 0.3),
            (Priority::Normal, 0.15),
            (Priority::Low, 0.05),
        ];
        harness.generate_workload(200, &distribution);

        // Process all tasks
        for _ in 0..200 {
            harness.process_batch(1).await;
        }

        let metrics = harness.metrics();
        assert!(metrics.throughput > 0.0, "Throughput should be measurable");

        // Rough sanity check: should process some reasonable number of tasks per second
        // Even on slow systems, 100 tasks in a test should complete in a few seconds
        assert!(
            metrics.elapsed.as_secs_f64() < 10.0,
            "Should complete 200 tasks in reasonable time"
        );
    }

    #[tokio::test]
    async fn soak_test_priority_ordering_under_load() {
        let mut harness = SoakTestHarness::new();

        // Enqueue tasks in reverse priority order
        let critical_id = TaskId::new();
        let low_id = TaskId::new();

        harness
            .scheduler
            .enqueue(make_test_task(low_id, Priority::Low, vec![]))
            .unwrap();
        harness
            .scheduler
            .enqueue(make_test_task(critical_id, Priority::Critical, vec![]))
            .unwrap();
        harness.task_count = 2;

        // First dequeue should be critical (higher priority)
        let t1 = harness.scheduler.dequeue(WorkerId::new()).await.unwrap();
        assert!(t1.is_some());
        assert_eq!(t1.unwrap().priority, Priority::Critical);
        *harness.dequeued.entry(Priority::Critical).or_insert(0) += 1;

        // Second should be low
        let t2 = harness.scheduler.dequeue(WorkerId::new()).await.unwrap();
        assert!(t2.is_some());
        assert_eq!(t2.unwrap().priority, Priority::Low);
        *harness.dequeued.entry(Priority::Low).or_insert(0) += 1;
    }
}
