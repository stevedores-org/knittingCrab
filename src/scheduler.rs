use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::dag::DagEngine;
use crate::lease::{Lease, LeaseManager};
use crate::resource_model::ResourceModel;
use crate::work_item::{TaskState, WorkItem};

pub struct SchedulerInner {
    pub queue: BinaryHeap<WorkItem>,
    pub dag: DagEngine,
    pub resources: ResourceModel,
    pub leases: LeaseManager,
    pub goal_locks: HashMap<String, u64>,
    pub cache: HashMap<String, TaskState>,
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
            })),
        }
    }

    pub fn enqueue(&self, item: WorkItem) -> Result<(), String> {
        let mut inner = self.inner.write().unwrap();
        inner.dag.add_task(item.clone())?;
        inner.queue.push(item);
        Ok(())
    }

    /// Returns the highest-priority task that has all dependencies met and sufficient resources.
    pub fn next_task(&self) -> Option<WorkItem> {
        let mut inner = self.inner.write().unwrap();
        let ready_ids: std::collections::HashSet<u64> = inner.dag.ready_tasks().into_iter().collect();

        // Drain the heap to find the best eligible task, then put the rest back.
        let mut candidates: Vec<WorkItem> = Vec::new();
        let mut chosen: Option<WorkItem> = None;

        while let Some(item) = inner.queue.pop() {
            if !ready_ids.contains(&item.id) {
                candidates.push(item);
                continue;
            }
            if inner.resources.can_allocate(item.required_cpu_cores, item.required_ram_mb, item.requires_gpu) {
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
    ) -> Result<Lease, String> {
        let mut inner = self.inner.write().unwrap();
        inner.resources.allocate(task.required_cpu_cores, task.required_ram_mb, task.requires_gpu)?;
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
        );
        Ok(lease)
    }

    pub fn complete_task(&self, lease_id: u64, success: bool, reason: Option<String>) {
        let mut inner = self.inner.write().unwrap();
        if let Some(lease) = inner.leases.revoke_lease(lease_id) {
            inner.resources.release(lease.cpu_cores, lease.ram_mb, lease.gpu);
            if success {
                inner.dag.mark_success(lease.task_id);
            } else {
                inner.dag.mark_failed(lease.task_id, reason.unwrap_or_else(|| "No failure reason provided".into()));
            }
        }
    }

    /// Re-queues tasks whose leases have expired.
    pub fn requeue_expired_leases(&self) {
        let mut inner = self.inner.write().unwrap();
        let expired = inner.leases.expired_leases();
        for lease in expired {
            inner.leases.revoke_lease(lease.id);
            inner.resources.release(lease.cpu_cores, lease.ram_mb, lease.gpu);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::Priority;

    fn make_scheduler() -> Scheduler {
        Scheduler::new(ResourceModel::new(8, 8192, 1, 4))
    }

    fn make_task(id: u64, priority: Priority, deps: Vec<u64>) -> WorkItem {
        WorkItem::new(id, format!("goal{}", id), "repo", "main", priority, deps, 1, 512, false, None, 3)
    }

    #[test]
    fn enqueue_and_dequeue() {
        let sched = make_scheduler();
        sched.enqueue(make_task(1, Priority::Batch, vec![])).unwrap();
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
        // After release, a third agent should be able to acquire
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
        let lease = sched.schedule_task(next, 1, Duration::from_secs(60)).unwrap();
        sched.complete_task(lease.id, true, None);
        // Resources should be released; we can allocate again
        {
            let inner = sched.inner.read().unwrap();
            assert_eq!(inner.resources.available_cpu_cores, inner.resources.total_cpu_cores);
        }
    }

    #[test]
    fn requeue_expired_leases_works() {
        let sched = make_scheduler();
        let task = make_task(1, Priority::Batch, vec![]);
        sched.enqueue(task.clone()).unwrap();
        let next = sched.next_task().unwrap();
        let lease = sched.schedule_task(next, 1, Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        sched.requeue_expired_leases();
        // Task should be back in the queue
        let requeued = sched.next_task();
        assert!(requeued.is_some());
        assert_eq!(requeued.unwrap().id, lease.task_id);
    }
}
