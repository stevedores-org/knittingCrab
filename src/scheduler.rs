use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::dag::DagEngine;
use crate::error::SchedulerError;
use crate::lease::{Lease, LeaseManager};
use crate::resource_model::ResourceModel;
use crate::work_item::{TaskState, WorkItem};

struct SchedulerInner {
    queue: BinaryHeap<WorkItem>,
    dag: DagEngine,
    resources: ResourceModel,
    leases: LeaseManager,
    goal_locks: HashMap<String, u64>,
    cache: HashMap<String, TaskState>,
    /// Exit codes that are eligible for retry. If empty, all failures are retryable.
    retryable_exit_codes: Vec<i32>,
}

pub struct Scheduler {
    inner: Arc<RwLock<SchedulerInner>>,
}

impl Scheduler {
    pub fn new(resources: ResourceModel) -> Self {
        Scheduler {
            inner: Arc::new(RwLock::new(SchedulerInner {
                queue: BinaryHeap::new(),
                dag: DagEngine::new(),
                resources,
                leases: LeaseManager::new(),
                goal_locks: HashMap::new(),
                cache: HashMap::new(),
                retryable_exit_codes: vec![],
            })),
        }
    }

    pub fn with_retryable_exit_codes(self, codes: Vec<i32>) -> Self {
        {
            let mut inner = self.inner.write().unwrap();
            inner.retryable_exit_codes = codes;
        }
        self
    }

    pub fn enqueue(&self, item: WorkItem) -> Result<(), SchedulerError> {
        let mut inner = self.inner.write().unwrap();
        inner.dag.add_task(item.clone())?;
        inner.queue.push(item);
        Ok(())
    }

    /// Returns the highest-priority task that has all dependencies met and sufficient resources.
    pub fn next_task(&self) -> Option<WorkItem> {
        let mut inner = self.inner.write().unwrap();
        let ready_ids: std::collections::HashSet<u64> =
            inner.dag.ready_tasks().into_iter().collect();

        // Drain the heap to find the best eligible task, then put the rest back.
        let mut candidates: Vec<WorkItem> = Vec::new();
        let mut chosen: Option<WorkItem> = None;

        while let Some(item) = inner.queue.pop() {
            if !ready_ids.contains(&item.id) {
                candidates.push(item);
                continue;
            }
            if inner.resources.can_allocate(
                item.required_cpu_cores,
                item.required_ram_mb,
                item.requires_gpu,
            ) {
                chosen = Some(item);
                break;
            }
            candidates.push(item);
        }

        for item in candidates {
            inner.queue.push(item);
        }

        if let Some(ref task) = chosen {
            if let Some(dag_task) = inner.dag.get_task_mut(task.id) {
                dag_task.state = TaskState::Scheduled;
            }
        }

        chosen
    }

    pub fn schedule_task(
        &self,
        task: WorkItem,
        worker_id: u64,
        lease_duration: Duration,
    ) -> Result<Lease, SchedulerError> {
        let mut inner = self.inner.write().unwrap();
        inner.resources.allocate(
            task.required_cpu_cores,
            task.required_ram_mb,
            task.requires_gpu,
        )?;
        if let Some(dag_task) = inner.dag.get_task_mut(task.id) {
            dag_task.state = TaskState::Running;
        }
        let lease = inner.leases.grant_lease(
            task.id,
            worker_id,
            task.required_cpu_cores,
            task.required_ram_mb,
            task.requires_gpu,
            lease_duration,
        )?;
        Ok(lease)
    }

    /// Completes a task. On failure, retries if the exit code is retryable and
    /// retries remain; otherwise marks the task as permanently failed.
    pub fn complete_task(
        &self,
        lease_id: u64,
        success: bool,
        exit_code: Option<i32>,
        reason: Option<String>,
    ) {
        let mut inner = self.inner.write().unwrap();
        if let Some(lease) = inner.leases.revoke_lease(lease_id) {
            inner
                .resources
                .release(lease.cpu_cores, lease.ram_mb, lease.gpu);
            if success {
                inner.dag.mark_success(lease.task_id);
            } else {
                let is_retryable = Self::is_retryable(&inner.retryable_exit_codes, exit_code);
                let can_retry = if let Some(task) = inner.dag.get_task_mut(lease.task_id) {
                    is_retryable && task.retry_count < task.max_retries
                } else {
                    false
                };

                if can_retry {
                    if let Some(task) = inner.dag.get_task_mut(lease.task_id) {
                        task.retry_count += 1;
                        task.state = TaskState::Pending;
                        let cloned = task.clone();
                        inner.queue.push(cloned);
                    }
                } else {
                    inner.dag.mark_failed(
                        lease.task_id,
                        reason.unwrap_or_else(|| "No failure reason provided".into()),
                    );
                }
            }
        }
    }

    /// Re-queues tasks whose leases have expired.
    pub fn requeue_expired_leases(&self) {
        let mut inner = self.inner.write().unwrap();
        let expired = inner.leases.expired_leases();
        for lease in expired {
            inner.leases.revoke_lease(lease.id);
            inner
                .resources
                .release(lease.cpu_cores, lease.ram_mb, lease.gpu);
            if let Some(task) = inner.dag.get_task_mut(lease.task_id) {
                task.state = TaskState::Pending;
                let cloned = task.clone();
                inner.queue.push(cloned);
            }
        }
    }

    /// Acquires an exclusive lock for a goal string. Returns true if successful.
    pub fn acquire_goal_lock(&self, goal: &str, task_id: u64) -> bool {
        let mut inner = self.inner.write().unwrap();
        if inner.goal_locks.contains_key(goal) {
            return false;
        }
        inner.goal_locks.insert(goal.to_string(), task_id);
        true
    }

    pub fn release_goal_lock(&self, goal: &str) {
        let mut inner = self.inner.write().unwrap();
        inner.goal_locks.remove(goal);
    }

    pub fn check_cache(&self, cache_key: &str) -> Option<TaskState> {
        let inner = self.inner.read().unwrap();
        inner.cache.get(cache_key).cloned()
    }

    pub fn store_cache(&self, cache_key: String, state: TaskState) {
        let mut inner = self.inner.write().unwrap();
        inner.cache.insert(cache_key, state);
    }

    pub fn resource_utilization(&self) -> f64 {
        let inner = self.inner.read().unwrap();
        inner.resources.utilization()
    }

    fn is_retryable(retryable_codes: &[i32], exit_code: Option<i32>) -> bool {
        if retryable_codes.is_empty() {
            return true;
        }
        match exit_code {
            Some(code) => retryable_codes.contains(&code),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::Priority;

    fn make_scheduler() -> Scheduler {
        Scheduler::new(ResourceModel::new(8, 8192, 1, 4))
    }

    fn make_task(id: u64, priority: Priority, deps: Vec<u64>) -> WorkItem {
        WorkItem::new(
            id,
            format!("goal{}", id),
            "repo",
            "main",
            priority,
            deps,
            1,
            512,
            false,
            None,
            3,
        )
    }

    #[test]
    fn enqueue_and_dequeue() {
        let sched = make_scheduler();
        sched
            .enqueue(make_task(1, Priority::Batch, vec![]))
            .unwrap();
        let task = sched.next_task();
        assert!(task.is_some());
    }

    #[test]
    fn two_agents_same_goal_one_wins_lock() {
        let sched = make_scheduler();
        let first = sched.acquire_goal_lock("deploy", 1);
        let second = sched.acquire_goal_lock("deploy", 2);
        assert!(first);
        assert!(!second);
        sched.release_goal_lock("deploy");
        let third = sched.acquire_goal_lock("deploy", 3);
        assert!(third);
    }

    #[test]
    fn cached_success_skips_execution() {
        let sched = make_scheduler();
        let task = make_task(1, Priority::Batch, vec![]);
        let key = task.cache_key.clone();
        sched.store_cache(key.clone(), TaskState::Success);
        let cached = sched.check_cache(&key);
        assert_eq!(cached, Some(TaskState::Success));
    }

    #[test]
    fn complete_task_releases_resources() {
        let sched = make_scheduler();
        let task = make_task(1, Priority::Batch, vec![]);
        sched.enqueue(task.clone()).unwrap();
        let next = sched.next_task().unwrap();
        let lease = sched
            .schedule_task(next, 1, Duration::from_secs(60))
            .unwrap();
        sched.complete_task(lease.id, true, None, None);
        assert!((sched.resource_utilization() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn requeue_expired_leases_works() {
        let sched = make_scheduler();
        let task = make_task(1, Priority::Batch, vec![]);
        sched.enqueue(task.clone()).unwrap();
        let next = sched.next_task().unwrap();
        let lease = sched
            .schedule_task(next, 1, Duration::from_millis(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        sched.requeue_expired_leases();
        let requeued = sched.next_task();
        assert!(requeued.is_some());
        assert_eq!(requeued.unwrap().id, lease.task_id);
    }

    #[test]
    fn duplicate_lease_prevention() {
        let sched = make_scheduler();
        let task = make_task(1, Priority::Batch, vec![]);
        sched.enqueue(task.clone()).unwrap();
        let next = sched.next_task().unwrap();
        let _lease = sched
            .schedule_task(next.clone(), 1, Duration::from_secs(60))
            .unwrap();
        let result = sched.schedule_task(next, 2, Duration::from_secs(60));
        assert!(matches!(
            result.unwrap_err(),
            SchedulerError::LeaseConflict { .. }
        ));
    }

    #[test]
    fn permanent_fail_after_max_attempts() {
        let sched = make_scheduler();
        let mut task = make_task(1, Priority::Batch, vec![]);
        task.max_retries = 2;
        sched.enqueue(task).unwrap();

        for attempt in 0..3 {
            let next = sched.next_task().unwrap();
            assert_eq!(next.id, 1, "attempt {}", attempt);
            let lease = sched
                .schedule_task(next, 1, Duration::from_secs(60))
                .unwrap();
            sched.complete_task(lease.id, false, Some(1), Some("oops".into()));
        }
        assert!(
            sched.next_task().is_none(),
            "Task should be permanently failed after max retries"
        );
    }

    #[test]
    fn retry_only_on_retryable_exit_codes() {
        let sched =
            Scheduler::new(ResourceModel::new(8, 8192, 1, 4)).with_retryable_exit_codes(vec![75]);

        let mut task = make_task(1, Priority::Batch, vec![]);
        task.max_retries = 3;
        sched.enqueue(task).unwrap();

        let next = sched.next_task().unwrap();
        let lease = sched
            .schedule_task(next, 1, Duration::from_secs(60))
            .unwrap();
        sched.complete_task(lease.id, false, Some(1), Some("bad exit".into()));

        assert!(
            sched.next_task().is_none(),
            "Non-retryable exit code should cause permanent failure"
        );

        let sched2 =
            Scheduler::new(ResourceModel::new(8, 8192, 1, 4)).with_retryable_exit_codes(vec![75]);
        let mut task2 = make_task(2, Priority::Batch, vec![]);
        task2.max_retries = 3;
        sched2.enqueue(task2).unwrap();

        let next2 = sched2.next_task().unwrap();
        let lease2 = sched2
            .schedule_task(next2, 1, Duration::from_secs(60))
            .unwrap();
        sched2.complete_task(lease2.id, false, Some(75), Some("retryable".into()));

        assert!(
            sched2.next_task().is_some(),
            "Retryable exit code should re-queue the task"
        );
    }

    #[test]
    fn cached_failure_policy_configurable() {
        let sched = make_scheduler();
        let task = make_task(1, Priority::Batch, vec![]);
        let key = task.cache_key.clone();

        sched.store_cache(key.clone(), TaskState::Failed("previous run".into()));
        let cached = sched.check_cache(&key);
        assert!(matches!(cached, Some(TaskState::Failed(_))));

        sched.store_cache(key.clone(), TaskState::Pending);
        let cleared = sched.check_cache(&key);
        assert_eq!(cleared, Some(TaskState::Pending));
    }

    #[test]
    fn resource_constrained_scheduling_skips_heavy_task() {
        // Only 2 CPU cores available — heavy task (4 cores) should be skipped
        // in favor of a lighter task (1 core) even though heavy has higher priority.
        let sched = Scheduler::new(ResourceModel::new(2, 8192, 1, 4));

        let mut heavy = WorkItem::new(
            1,
            "heavy",
            "repo",
            "main",
            Priority::Interactive,
            vec![],
            4,
            512,
            false,
            None,
            0,
        );
        heavy.required_cpu_cores = 4; // needs 4 but only 2 available

        let light = WorkItem::new(
            2,
            "light",
            "repo",
            "main",
            Priority::Batch,
            vec![],
            1,
            512,
            false,
            None,
            0,
        );

        sched.enqueue(heavy).unwrap();
        sched.enqueue(light).unwrap();

        // next_task should skip the heavy Interactive task and pick the light Batch task
        let next = sched.next_task().unwrap();
        assert_eq!(
            next.id, 2,
            "Should schedule lighter task when heavy task exceeds resources"
        );
        assert_eq!(next.priority, Priority::Batch);
    }
}
