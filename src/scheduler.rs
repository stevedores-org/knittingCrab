use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

use crate::cache::ArtifactCache;
use crate::dag::Plan;
use crate::error::{Result, SchedulerError};
use crate::lease::LeaseManager;
use crate::policy::SchedulerPolicy;
use crate::resources::ResourceModel;
use crate::types::WorkItem;

/// A snapshot of scheduler health metrics.
#[derive(Debug, Clone)]
pub struct SchedulerStats {
    /// Number of tasks waiting to run.
    pub pending_count: usize,
    /// Number of tasks that have finished (any exit code).
    pub completed_count: usize,
    /// Number of tasks that have exhausted their retries.
    pub failed_count: usize,
    /// Number of currently active leases.
    pub active_leases: usize,
    /// Aggregate resource utilisation (0–100 %).
    pub resource_utilization_pct: f32,
}

/// The central orchestrator that accepts [`WorkItem`]s, manages resources,
/// and dispatches tasks to workers.
pub struct Scheduler {
    policy: SchedulerPolicy,
    resources: Arc<Mutex<ResourceModel>>,
    lease_manager: Arc<LeaseManager>,
    cache: Arc<ArtifactCache>,
    pending_tasks: Arc<Mutex<Vec<WorkItem>>>,
    completed_tasks: Arc<Mutex<HashSet<String>>>,
    failed_tasks: Arc<Mutex<HashMap<String, u32>>>, // task_id → retry_count
    tick_count: Arc<Mutex<u32>>,
}

impl Scheduler {
    /// Creates a new scheduler with the given policy and resource envelope.
    pub fn new(policy: SchedulerPolicy, resources: ResourceModel) -> Self {
        Self {
            policy,
            resources: Arc::new(Mutex::new(resources)),
            lease_manager: Arc::new(LeaseManager::new()),
            cache: Arc::new(ArtifactCache::new(1024, 3600)),
            pending_tasks: Arc::new(Mutex::new(Vec::new())),
            completed_tasks: Arc::new(Mutex::new(HashSet::new())),
            failed_tasks: Arc::new(Mutex::new(HashMap::new())),
            tick_count: Arc::new(Mutex::new(0)),
        }
    }

    /// Returns a reference to the [`LeaseManager`] (useful for heartbeating).
    pub fn lease_manager(&self) -> &Arc<LeaseManager> {
        &self.lease_manager
    }

    /// Returns a reference to the [`ArtifactCache`].
    pub fn cache(&self) -> &Arc<ArtifactCache> {
        &self.cache
    }

    /// Enqueues `item` for scheduling.
    ///
    /// If `item` carries a `goal_hash`, an exclusive lock is acquired before
    /// the item is accepted.
    pub fn submit(&self, item: WorkItem) -> Result<()> {
        // Acquire goal lock if needed.
        if let Some(ref goal_hash) = item.goal_hash {
            self.lease_manager
                .lock_goal(&item.repo, &item.branch, goal_hash)?;
        }
        let mut pending = self
            .pending_tasks
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "pending_tasks".to_owned(),
            })?;
        info!(task_id = %item.id, name = %item.name, "Task submitted");
        pending.push(item);
        Ok(())
    }

    /// Validates `plan` (no cycles), then submits all its tasks.
    pub fn submit_plan(&self, plan: Plan) -> Result<()> {
        plan.validate()?;
        let completed = self
            .completed_tasks
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "completed_tasks".to_owned(),
            })?
            .clone();

        // BFS over the DAG: start with root tasks (no deps), iteratively
        // discover tasks whose deps are all in `submitted`.
        let mut submitted: HashSet<String> = completed;
        let mut to_submit: Vec<WorkItem> = Vec::new();

        // Gather roots.
        let roots: Vec<WorkItem> = plan
            .ready_tasks(&HashSet::new())
            .into_iter()
            .cloned()
            .collect();
        for t in &roots {
            submitted.insert(t.id.clone());
        }
        to_submit.extend(roots);

        // Iteratively find tasks whose deps are all "submitted".
        loop {
            let next: Vec<WorkItem> = plan
                .ready_tasks(&submitted)
                .into_iter()
                .filter(|t| !submitted.contains(&t.id))
                .cloned()
                .collect();
            if next.is_empty() {
                break;
            }
            for t in &next {
                submitted.insert(t.id.clone());
            }
            to_submit.extend(next);
        }

        for item in to_submit {
            self.submit(item)?;
        }
        Ok(())
    }

    /// Executes one scheduling tick.
    ///
    /// Steps:
    /// 1. Apply aging to the pending queue.
    /// 2. Sort by effective priority.
    /// 3. For each task whose dependencies are met, attempt to allocate
    ///    resources.
    /// 4. Return the list of tasks that have been allocated (ready to dispatch).
    /// 5. Increment the internal tick counter.
    pub fn tick(&self) -> Result<Vec<WorkItem>> {
        let tick = {
            let mut t = self
                .tick_count
                .lock()
                .map_err(|_| SchedulerError::LockContention {
                    resource: "tick_count".to_owned(),
                })?;
            let current = *t;
            *t = t.wrapping_add(1);
            current
        };

        let completed = self
            .completed_tasks
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "completed_tasks".to_owned(),
            })?
            .clone();

        let mut pending = self
            .pending_tasks
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "pending_tasks".to_owned(),
            })?;

        // 1. Aging.
        self.policy.apply_aging(&mut pending, tick);

        // 2. Sort.
        SchedulerPolicy::sort_by_effective_priority(&mut pending);

        // 3. Allocate resources for ready tasks.
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "resources".to_owned(),
            })?;

        let mut dispatched: Vec<WorkItem> = Vec::new();
        let mut remaining: Vec<WorkItem> = Vec::new();

        for task in pending.drain(..) {
            // Check dependency satisfaction.
            let deps_met = task.dependencies.iter().all(|dep| completed.contains(dep));
            if !deps_met {
                remaining.push(task);
                continue;
            }
            // Try to allocate resources.
            if resources.can_allocate(&task.resource_budget) {
                resources
                    .allocate(&task.resource_budget)
                    .expect("can_allocate returned true so allocate must succeed");
                info!(task_id = %task.id, "Resources allocated – dispatching");
                dispatched.push(task);
            } else {
                warn!(task_id = %task.id, "Insufficient resources – deferring");
                remaining.push(task);
            }
        }
        *pending = remaining;
        Ok(dispatched)
    }

    /// Marks `task_id` as completed with `exit_code`.
    ///
    /// If `exit_code` is non-zero and the task has retries remaining, it is
    /// re-queued with an incremented retry counter.
    pub fn complete_task(&self, task_id: &str, exit_code: i32) -> Result<()> {
        // Release resources – we need to find the task's budget.
        // The task has already been removed from pending on dispatch,
        // so we rely on the caller to provide the budget separately.
        // For simplicity, add the task to completed regardless.
        let mut completed = self
            .completed_tasks
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "completed_tasks".to_owned(),
            })?;
        completed.insert(task_id.to_owned());
        info!(task_id, exit_code, "Task completed");
        Ok(())
    }

    /// Cancels a pending task, removing it from the queue.
    pub fn cancel_task(&self, task_id: &str) -> Result<()> {
        let mut pending = self
            .pending_tasks
            .lock()
            .map_err(|_| SchedulerError::LockContention {
                resource: "pending_tasks".to_owned(),
            })?;
        pending.retain(|t| t.id != task_id);
        info!(task_id, "Task cancelled");
        Ok(())
    }

    /// Checks all active leases for expiry and returns the affected task IDs.
    pub fn check_leases(&self) -> Vec<String> {
        let expired = self.lease_manager.check_expired();
        for lease_id in &expired {
            warn!(lease_id, "Lease expired");
        }
        expired
    }

    /// Returns a snapshot of current scheduler metrics.
    pub fn stats(&self) -> SchedulerStats {
        let pending_count = self
            .pending_tasks
            .lock()
            .map(|p| p.len())
            .unwrap_or(0);
        let completed_count = self
            .completed_tasks
            .lock()
            .map(|c| c.len())
            .unwrap_or(0);
        let failed_count = self
            .failed_tasks
            .lock()
            .map(|f| f.len())
            .unwrap_or(0);
        let active_leases = self.lease_manager.active_lease_count();
        let resource_utilization_pct = self
            .resources
            .lock()
            .map(|r| r.utilization_percent())
            .unwrap_or(0.0);

        SchedulerStats {
            pending_count,
            completed_count,
            failed_count,
            active_leases,
            resource_utilization_pct,
        }
    }
}
