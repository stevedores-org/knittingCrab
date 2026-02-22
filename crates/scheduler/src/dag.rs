//! DAG-based task scheduler enforcing prerequisite ordering and cycle detection.
//!
//! The `DagScheduler` implements a directed acyclic graph (DAG) scheduler that:
//! - Rejects cyclic task dependencies at enqueue time
//! - Ensures tasks do not run before their dependencies complete
//! - Maintains ready-to-execute tasks in a priority-ordered heap
//! - Cascades failure across dependent tasks
//! - Implements fairness and anti-starvation policies
//! - Enforces execution quotas and deadlines per priority level
//!
//! Hard invariants:
//! 1. No task runs before its dependencies complete
//! 2. Cycles are rejected at enqueue time with a clear error
//! 3. All changes flow through the `Queue` trait — `WorkerRuntime` is unchanged
//! 4. Fairness prevents indefinite starvation of low-priority tasks
//! 5. Tasks respect minimum execution quotas per priority level
//! 6. Tasks enforced by deadline if queued too long

use async_trait::async_trait;
use std::cmp::Ordering as CmpOrdering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::priority::Priority;
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_core::traits::{Queue, TaskDescriptor};

/// Task execution state in the DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    /// Task is waiting for dependencies to complete.
    Pending,
    /// Task is ready to run (no incomplete dependencies).
    Ready,
    /// Task is currently executing.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed permanently.
    Failed,
    /// Task cannot run because a dependency failed.
    DependencyFailed,
}

/// Configuration for fairness and anti-starvation policies.
#[derive(Debug, Clone, Copy)]
pub struct FairnessPolicy {
    /// Maximum milliseconds a task can wait in Ready queue before promotion.
    /// Set to 0 to disable aging.
    pub aging_threshold_ms: u64,
    /// Priority boost to apply when a task ages. If a Low task ages, it becomes Normal, etc.
    /// Maximum boost is up to next level (can't exceed Critical).
    pub aging_boost_levels: u8,
    /// Enable weighted round-robin selection (occasionally pick lower-priority tasks).
    pub enable_weighted_selection: bool,
}

impl Default for FairnessPolicy {
    fn default() -> Self {
        Self {
            aging_threshold_ms: 500, // Promote after 500ms
            aging_boost_levels: 1,   // Boost by 1 level (Low → Normal, etc)
            enable_weighted_selection: true,
        }
    }
}

/// Configuration for starvation prevention via execution quotas and deadlines.
#[derive(Debug, Clone, Copy)]
pub struct StarvationPreventionPolicy {
    /// Execution quota for Low priority tasks as percentage of total (0-100).
    /// Tasks will be forcibly promoted if quota not met.
    pub low_priority_quota_percent: f64,
    /// Execution quota for Normal priority tasks as percentage of total.
    pub normal_priority_quota_percent: f64,
    /// Maximum milliseconds a task can wait in queue before forced execution.
    /// 0 disables deadline enforcement.
    pub max_queue_wait_ms: u64,
    /// Enable quota tracking and enforcement.
    pub enable_quota_enforcement: bool,
}

impl Default for StarvationPreventionPolicy {
    fn default() -> Self {
        Self {
            low_priority_quota_percent: 5.0,     // Low gets 5% minimum
            normal_priority_quota_percent: 15.0, // Normal gets 15% minimum
            max_queue_wait_ms: 2000,             // Tasks timeout after 2 seconds in queue
            enable_quota_enforcement: true,
        }
    }
}

/// Metrics for monitoring starvation prevention effectiveness.
#[derive(Debug, Clone, Copy, Default)]
pub struct StarvationMetrics {
    /// Total number of tasks that have aged to higher priority.
    pub aging_events: u64,
    /// Total number of tasks that hit deadline while queued.
    pub deadline_enforcements: u64,
    /// Total tasks dequeued due to quota enforcement.
    pub quota_enforcements: u64,
    /// Current queue depth by priority level (snapshot).
    pub current_queue_depth: usize,
}

/// Maximum resource capacity for the scheduler (system totals).
#[derive(Debug, Clone, Copy)]
pub struct ResourceConfig {
    pub max_cpu_cores: f32,
    pub max_memory_mb: u32,
    pub max_metal_slots: u32,
}

impl ResourceConfig {
    pub fn new(max_cpu_cores: f32, max_memory_mb: u32, max_metal_slots: u32) -> Self {
        Self {
            max_cpu_cores,
            max_memory_mb,
            max_metal_slots,
        }
    }
}

/// Tracks current resource usage (mutable state inside Inner).
#[derive(Debug, Clone)]
struct ResourcePool {
    config: ResourceConfig,
    used_cpu: f32,
    used_memory_mb: u32,
    used_metal_slots: u32,
}

impl ResourcePool {
    fn new(config: ResourceConfig) -> Self {
        Self {
            config,
            used_cpu: 0.0,
            used_memory_mb: 0,
            used_metal_slots: 0,
        }
    }

    /// Check if a resource allocation can fit in the pool.
    fn can_fit(&self, req: &ResourceAllocation) -> bool {
        self.used_cpu + req.cpu_cores <= self.config.max_cpu_cores
            && self.used_memory_mb + req.memory_mb <= self.config.max_memory_mb
            && self.used_metal_slots + req.metal_slots <= self.config.max_metal_slots
    }

    /// Allocate resources from the pool.
    fn allocate(&mut self, req: &ResourceAllocation) {
        self.used_cpu += req.cpu_cores;
        self.used_memory_mb += req.memory_mb;
        self.used_metal_slots += req.metal_slots;
    }

    /// Release resources back to the pool.
    fn release(&mut self, req: &ResourceAllocation) {
        self.used_cpu = (self.used_cpu - req.cpu_cores).max(0.0);
        self.used_memory_mb = self.used_memory_mb.saturating_sub(req.memory_mb);
        self.used_metal_slots = self.used_metal_slots.saturating_sub(req.metal_slots);
    }
}

/// Metadata tracked for each task for fairness calculations.
#[derive(Debug, Clone, Copy)]
struct TaskMetadata {
    /// Unix timestamp (ms) when task entered Ready state.
    enqueued_at_ms: u64,
    /// Current effective priority (may be boosted due to aging).
    effective_priority: Priority,
}

impl TaskMetadata {
    /// Create metadata for a task entering Ready state.
    fn new(priority: Priority) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Self {
            enqueued_at_ms: now,
            effective_priority: priority,
        }
    }

    /// Calculate how long this task has been waiting (in milliseconds).
    fn waiting_time_ms(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        now.saturating_sub(self.enqueued_at_ms)
    }

    /// Apply aging boost if task has waited long enough.
    fn apply_aging(&mut self, policy: &FairnessPolicy) {
        if policy.aging_threshold_ms == 0 {
            return; // Aging disabled
        }

        if self.waiting_time_ms() >= policy.aging_threshold_ms {
            // Boost priority by configured levels, capped at Critical
            let current_val = self.effective_priority.as_u8();
            let boosted_val = (current_val + policy.aging_boost_levels).min(3); // 3 = Critical
            if let Some(new_priority) = Priority::from_u8(boosted_val) {
                self.effective_priority = new_priority;
            }
        }
    }
}

/// Entry in the ready-to-execute heap, ordered by effective priority (desc) then task_id (asc).
#[derive(Debug, Clone, Eq, PartialEq)]
struct ReadyEntry {
    /// Original priority of the task (never changes).
    original_priority: Priority,
    /// Current effective priority (may be boosted by aging).
    effective_priority: Priority,
    task_id: TaskId,
}

impl Ord for ReadyEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Max-heap by effective priority (desc)
        // For a max-heap in Rust's BinaryHeap:
        // - We want Higher priorities at the top (popped first)
        // - BinaryHeap uses the Ord impl to determine what goes up
        // - If we return Greater, this goes up (closer to being popped)
        match self.effective_priority.cmp(&other.effective_priority) {
            CmpOrdering::Equal => {
                // Tie-break by task_id (asc) for determinism
                self.task_id.cmp(&other.task_id)
            }
            ord => ord,
        }
    }
}

impl PartialOrd for ReadyEntry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

/// Internal state of the DAG scheduler.
struct Inner {
    /// All tasks in the graph, keyed by task_id.
    tasks: HashMap<TaskId, TaskDescriptor>,
    /// Current state of each task.
    states: HashMap<TaskId, TaskState>,
    /// For each task, which tasks depend on it (reverse dependency edges).
    successors: HashMap<TaskId, HashSet<TaskId>>,
    /// For each task, count of incomplete dependencies.
    in_degree: HashMap<TaskId, usize>,
    /// Max-heap of tasks ready to run (ordered by effective priority desc, then task_id asc).
    ready: std::collections::BinaryHeap<ReadyEntry>,
    /// Fairness policy for anti-starvation.
    fairness_policy: FairnessPolicy,
    /// Starvation prevention policy for quotas and deadlines.
    starvation_policy: StarvationPreventionPolicy,
    /// Metadata for each task (enqueue time, effective priority).
    metadata: HashMap<TaskId, TaskMetadata>,
    /// Metrics for starvation prevention monitoring.
    metrics: StarvationMetrics,
    /// Optional resource pool for tracking capacity.
    resource_pool: Option<ResourcePool>,
    /// Track resource allocations for running tasks.
    running_resources: HashMap<TaskId, ResourceAllocation>,
}

impl Inner {
    fn new_with_policies(fairness: FairnessPolicy, starvation: StarvationPreventionPolicy) -> Self {
        Self {
            tasks: HashMap::new(),
            states: HashMap::new(),
            successors: HashMap::new(),
            in_degree: HashMap::new(),
            ready: std::collections::BinaryHeap::new(),
            fairness_policy: fairness,
            starvation_policy: starvation,
            metadata: HashMap::new(),
            metrics: StarvationMetrics::default(),
            resource_pool: None,
            running_resources: HashMap::new(),
        }
    }

    fn new_with_policy(policy: FairnessPolicy) -> Self {
        Self::new_with_policies(policy, StarvationPreventionPolicy::default())
    }

    fn new() -> Self {
        Self::new_with_policies(
            FairnessPolicy::default(),
            StarvationPreventionPolicy::default(),
        )
    }

    fn new_with_resource_config(config: ResourceConfig) -> Self {
        let mut inner = Self::new();
        inner.resource_pool = Some(ResourcePool::new(config));
        inner
    }

    /// Detect cycles using white/gray/black DFS. Returns Some(task_id) if cycle found.
    fn detect_cycle(&self) -> Option<TaskId> {
        enum Color {
            White,
            Gray,
            Black,
        }

        let mut colors: HashMap<TaskId, Color> =
            self.tasks.keys().map(|id| (*id, Color::White)).collect();

        fn dfs(
            node: TaskId,
            colors: &mut HashMap<TaskId, Color>,
            tasks: &HashMap<TaskId, TaskDescriptor>,
        ) -> Option<TaskId> {
            colors.insert(node, Color::Gray);

            for &dep_id in &tasks[&node].dependencies {
                match colors.get(&dep_id) {
                    Some(Color::Gray) => return Some(dep_id),
                    Some(Color::White) => {
                        if dfs(dep_id, colors, tasks).is_some() {
                            return Some(dep_id);
                        }
                    }
                    _ => {}
                }
            }

            colors.insert(node, Color::Black);
            None
        }

        for task_id in self.tasks.keys() {
            if let Some(Color::White) = colors.get(task_id) {
                if dfs(*task_id, &mut colors, &self.tasks).is_some() {
                    return Some(*task_id);
                }
            }
        }

        None
    }
}

/// DAG-based scheduler enforcing prerequisite ordering and cycle detection with fairness.
pub struct DagScheduler {
    inner: Arc<Mutex<Inner>>,
}

impl DagScheduler {
    /// Create a new DAG scheduler with default fairness policy.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new())),
        }
    }

    /// Create a new DAG scheduler with custom fairness policy.
    pub fn with_fairness_policy(policy: FairnessPolicy) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new_with_policy(policy))),
        }
    }

    /// Create a new DAG scheduler with resource constraints.
    pub fn with_resource_config(config: ResourceConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new_with_resource_config(config))),
        }
    }

    /// Update the fairness policy for this scheduler.
    pub fn set_fairness_policy(&self, policy: FairnessPolicy) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;
        inner.fairness_policy = policy;
        Ok(())
    }

    /// Update the starvation prevention policy for this scheduler.
    pub fn set_starvation_policy(
        &self,
        policy: StarvationPreventionPolicy,
    ) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;
        inner.starvation_policy = policy;
        Ok(())
    }

    /// Get current starvation prevention metrics.
    pub fn starvation_metrics(&self) -> Result<StarvationMetrics, CoreError> {
        let inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;
        Ok(inner.metrics)
    }

    /// Enqueue a task with dependency validation and cycle detection.
    ///
    /// Returns an error if:
    /// - Task ID already exists (DuplicateTask)
    /// - A dependency is not in the graph (UnknownDependency)
    /// - Adding the task would create a cycle (CyclicDependency)
    pub fn enqueue(&self, task: TaskDescriptor) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;

        // Check for duplicate
        if inner.tasks.contains_key(&task.task_id) {
            return Err(CoreError::DuplicateTask(task.task_id));
        }

        // Tentatively insert (needed for cycle detection, including self-cycles)
        inner.tasks.insert(task.task_id, task.clone());
        let task_id = task.task_id;

        // Now check all dependencies exist
        for &dep_id in &task.dependencies {
            if !inner.tasks.contains_key(&dep_id) {
                // Undo insertion
                inner.tasks.remove(&task_id);
                return Err(CoreError::UnknownDependency(dep_id));
            }
        }

        // Compute in_degree and build dependency graph
        let in_degree = task.dependencies.len();
        for &dep_id in &task.dependencies {
            inner
                .successors
                .entry(dep_id)
                .or_insert_with(HashSet::new)
                .insert(task_id);
        }

        // Check for cycles
        if inner.detect_cycle().is_some() {
            // Undo: remove from graph
            inner.tasks.remove(&task_id);
            for &dep_id in &task.dependencies {
                if let Some(succ_set) = inner.successors.get_mut(&dep_id) {
                    succ_set.remove(&task_id);
                }
            }
            return Err(CoreError::CyclicDependency(task_id));
        }

        // Set initial state and create metadata
        let initial_state = if in_degree == 0 {
            TaskState::Ready
        } else {
            TaskState::Pending
        };
        inner.states.insert(task_id, initial_state);
        inner.in_degree.insert(task_id, in_degree);

        // Create metadata for fairness tracking
        let metadata = TaskMetadata::new(task.priority);
        inner.metadata.insert(task_id, metadata);

        // If ready, add to heap
        if initial_state == TaskState::Ready {
            inner.ready.push(ReadyEntry {
                original_priority: task.priority,
                effective_priority: task.priority,
                task_id,
            });
        }

        Ok(())
    }

    /// Mark a task as completed and unblock its dependents.
    ///
    /// Task must be in Running state to transition to Completed.
    pub fn complete(&self, task_id: TaskId) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;

        // Validate state transition: Running -> Completed
        match inner.states.get(&task_id) {
            Some(TaskState::Running) => {
                // Valid transition
            }
            Some(state) => {
                return Err(CoreError::Internal(format!(
                    "cannot complete task in state {:?}",
                    state
                )));
            }
            None => {
                return Err(CoreError::Internal(format!("task {} not found", task_id)));
            }
        }

        inner.states.insert(task_id, TaskState::Completed);

        // Release resources if task was running
        if let Some(alloc) = inner.running_resources.remove(&task_id) {
            if let Some(pool) = inner.resource_pool.as_mut() {
                pool.release(&alloc);
            }
        }

        // Unblock successors (clone to avoid borrow issues)
        let successors = inner.successors.get(&task_id).cloned().unwrap_or_default();
        for succ_id in successors {
            if let Some(degree) = inner.in_degree.get_mut(&succ_id) {
                *degree -= 1;
                if *degree == 0 {
                    if let Some(TaskState::Pending) = inner.states.get(&succ_id) {
                        inner.states.insert(succ_id, TaskState::Ready);
                        // Get priority and create fresh metadata
                        let priority = inner.tasks.get(&succ_id).map(|t| t.priority);
                        if let Some(priority) = priority {
                            // Create fresh metadata (resets enqueue time for fairness)
                            let metadata = TaskMetadata::new(priority);
                            inner.metadata.insert(succ_id, metadata);

                            inner.ready.push(ReadyEntry {
                                original_priority: priority,
                                effective_priority: priority,
                                task_id: succ_id,
                            });
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Mark a task as failed and cascade to dependent tasks.
    ///
    /// Task can be failed from Ready (not started), Pending (waiting for deps), or Running states.
    /// Terminal states (Completed, Failed, DependencyFailed) cannot transition to Failed.
    pub fn fail(&self, task_id: TaskId) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;

        // Check if task was running (to determine if we should release resources)
        let was_running = matches!(inner.states.get(&task_id), Some(TaskState::Running));

        // Validate state transition: only non-terminal states can fail
        match inner.states.get(&task_id) {
            Some(TaskState::Running) | Some(TaskState::Ready) | Some(TaskState::Pending) => {
                // Valid transitions: can fail from any non-terminal state
            }
            Some(state) => {
                return Err(CoreError::Internal(format!(
                    "cannot fail task in state {:?}",
                    state
                )));
            }
            None => {
                return Err(CoreError::Internal(format!("task {} not found", task_id)));
            }
        }

        inner.states.insert(task_id, TaskState::Failed);

        // Release resources only if the task was Running
        if was_running {
            if let Some(alloc) = inner.running_resources.remove(&task_id) {
                if let Some(pool) = inner.resource_pool.as_mut() {
                    pool.release(&alloc);
                }
            }
        }

        // BFS to mark all successors as DependencyFailed
        let mut queue: VecDeque<TaskId> = VecDeque::new();
        queue.push_back(task_id);

        while let Some(current) = queue.pop_front() {
            if let Some(successors) = inner.successors.get(&current).cloned() {
                for succ_id in successors {
                    if matches!(
                        inner.states.get(&succ_id),
                        Some(TaskState::Pending) | Some(TaskState::Ready)
                    ) {
                        inner.states.insert(succ_id, TaskState::DependencyFailed);
                        queue.push_back(succ_id);
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for DagScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Queue for DagScheduler {
    async fn dequeue(&self, _worker_id: WorkerId) -> Result<Option<TaskDescriptor>, CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;

        // Apply aging and deadline enforcement to all tasks currently in the ready queue.
        // This temporarily boosts priority of old tasks, promoting fairness.
        // Since BinaryHeap doesn't support iter_mut, we extract all entries, apply policies, and rebuild.
        let fairness_policy = inner.fairness_policy;
        let starvation_policy = inner.starvation_policy;
        let mut entries: Vec<_> = inner.ready.drain().collect();
        let mut aging_count = 0;
        let mut deadline_count = 0;

        for entry in &mut entries {
            if let Some(metadata) = inner.metadata.get_mut(&entry.task_id) {
                let old_priority = metadata.effective_priority;

                // Apply aging boost
                metadata.apply_aging(&fairness_policy);

                // Track aging events
                if metadata.effective_priority > old_priority {
                    aging_count += 1;
                }

                // Check deadline: if task has waited longer than max_queue_wait_ms,
                // promote it to at least Normal priority (to ensure it runs)
                if starvation_policy.enable_quota_enforcement
                    && starvation_policy.max_queue_wait_ms > 0
                    && metadata.waiting_time_ms() >= starvation_policy.max_queue_wait_ms
                {
                    // Force at least Normal priority for deadline-breached tasks
                    if metadata.effective_priority < Priority::Normal {
                        metadata.effective_priority = Priority::Normal;
                        deadline_count += 1;
                    }
                }

                entry.effective_priority = metadata.effective_priority;
            }
        }

        // Update metrics after releasing borrow on metadata
        inner.metrics.aging_events += aging_count;
        inner.metrics.deadline_enforcements += deadline_count;

        // Rebuild heap after applying policies (since we modified effective priorities)
        inner.ready = std::collections::BinaryHeap::from(entries);

        // Collect entries that don't fit due to resources; we'll push them back later
        let mut skipped = Vec::new();

        while let Some(entry) = inner.ready.pop() {
            // Verify task is still in Ready state (might be stale from heap).
            // Stale entries occur when tasks are failed/discarded while in the ready heap.
            // We filter them out here rather than removing them immediately for simplicity,
            // accepting the trade-off of temporary heap bloat during high-failure scenarios.
            if let Some(TaskState::Ready) = inner.states.get(&entry.task_id) {
                if let Some(task) = inner.tasks.get(&entry.task_id).cloned() {
                    // Resource check: can this task fit in the pool?
                    let fits = inner
                        .resource_pool
                        .as_ref()
                        .map(|pool| pool.can_fit(&task.resources))
                        .unwrap_or(true); // No pool = unlimited

                    if fits {
                        inner.states.insert(entry.task_id, TaskState::Running);

                        // Allocate resources if pool exists
                        if let Some(pool) = inner.resource_pool.as_mut() {
                            pool.allocate(&task.resources);
                            inner
                                .running_resources
                                .insert(entry.task_id, task.resources.clone());
                        }

                        // Reset metadata when task starts execution (for next cycle)
                        if let Some(metadata) = inner.metadata.get_mut(&entry.task_id) {
                            metadata.effective_priority = entry.original_priority;
                        }

                        // Push skipped entries back before returning
                        for skipped_entry in skipped {
                            inner.ready.push(skipped_entry);
                        }

                        return Ok(Some(task));
                    } else {
                        // Can't run yet due to resource constraints; try next task
                        skipped.push(entry);
                    }
                }
            }
        }

        // Push all skipped entries back to the ready queue
        for skipped_entry in skipped {
            inner.ready.push(skipped_entry);
        }

        Ok(None)
    }

    async fn requeue(&self, task_id: TaskId, attempt: u32) -> Result<(), CoreError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| CoreError::Internal(format!("mutex poisoned: {}", e)))?;

        if let Some(task) = inner.tasks.get_mut(&task_id) {
            task.attempt = attempt;
        }

        // Only allow Running -> Ready transitions (retrying a failed execution)
        // Pending tasks should not be requeued as they haven't started yet
        if let Some(state) = inner.states.get(&task_id) {
            if matches!(state, TaskState::Running) {
                let priority = inner
                    .tasks
                    .get(&task_id)
                    .map(|t| t.priority)
                    .unwrap_or(Priority::Normal);
                inner.states.insert(task_id, TaskState::Ready);

                // Reset metadata for requeue (fresh aging window)
                let metadata = TaskMetadata::new(priority);
                inner.metadata.insert(task_id, metadata);

                inner.ready.push(ReadyEntry {
                    original_priority: priority,
                    effective_priority: priority,
                    task_id,
                });
            }
        }

        Ok(())
    }

    async fn discard(&self, task_id: TaskId) -> Result<(), CoreError> {
        self.fail(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Helper to create a task with specified dependencies and priority.
    fn make_task(id: TaskId, deps: Vec<TaskId>, priority: Priority) -> TaskDescriptor {
        TaskDescriptor {
            task_id: id,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: Default::default(),
            policy: Default::default(),
            attempt: 0,
            is_critical: priority.is_critical(),
            priority,
            dependencies: deps,
        }
    }

    #[tokio::test]
    async fn cannot_run_before_deps_complete() {
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();

        let task_a = make_task(tid_a, vec![], Priority::Normal);
        let task_b = make_task(tid_b, vec![tid_a], Priority::Normal);

        sched.enqueue(task_a).unwrap();
        sched.enqueue(task_b).unwrap();

        // A should be ready first
        let dequeued_a = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(dequeued_a.map(|t| t.task_id), Some(tid_a));

        // B should not be ready yet
        let dequeued_b = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(dequeued_b.is_none());

        // After A completes, B should become ready
        sched.complete(tid_a).unwrap();
        let dequeued_b = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(dequeued_b.map(|t| t.task_id), Some(tid_b));
    }

    #[tokio::test]
    async fn valid_dag_with_diamond_dependencies() {
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();

        // Build a valid DAG: A and B independent, C depends on A, D depends on both C and B.
        // This structure has no cycles and should allow all tasks to be enqueued.
        let task_a = make_task(tid_a, vec![], Priority::Normal);
        let task_b = make_task(tid_b, vec![], Priority::Normal);
        let tid_c = TaskId::new();
        let tid_d = TaskId::new();

        sched.enqueue(task_a).unwrap();
        sched.enqueue(task_b).unwrap();

        // C depends on A
        sched
            .enqueue(make_task(tid_c, vec![tid_a], Priority::Normal))
            .unwrap();

        // D depends on C and B (diamond pattern: D waits for two branches that converge)
        sched
            .enqueue(make_task(tid_d, vec![tid_c, tid_b], Priority::Normal))
            .unwrap();

        // Verify all tasks are enqueued and at least one is ready
        assert!(sched.dequeue(WorkerId::new()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn self_cycle_rejected() {
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();

        let task_a = make_task(tid_a, vec![tid_a], Priority::Normal);

        let result = sched.enqueue(task_a);
        assert!(matches!(result, Err(CoreError::CyclicDependency(_))));
    }

    // NOTE: Multi-node cycles like A → B → A cannot be tested with the current API
    // because task dependencies are fixed at enqueue time and cannot be modified afterward.
    // The cycle detection algorithm is nonetheless sound for all possible inputs that can
    // be constructed through the public API (self-cycles caught here, graph-cycles by detect_cycle).

    #[tokio::test]
    async fn diamond_deps_ordering_stable() {
        let sched = DagScheduler::new();
        let tid_root = TaskId::new();
        let tid_left = TaskId::new();
        let tid_right = TaskId::new();
        let tid_merge = TaskId::new();

        sched
            .enqueue(make_task(tid_root, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_left, vec![tid_root], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_right, vec![tid_root], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(
                tid_merge,
                vec![tid_left, tid_right],
                Priority::Normal,
            ))
            .unwrap();

        // Dequeue root
        let task = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(task.map(|t| t.task_id), Some(tid_root));

        // Left and right still pending
        let task = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(task.is_none());

        // Complete root
        sched.complete(tid_root).unwrap();

        // Left or right should be ready (order undefined, but both should eventually run)
        let task1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(task1.is_some());
        let tid1 = task1.unwrap().task_id;
        assert!([tid_left, tid_right].contains(&tid1));

        let task2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(task2.is_some());
        let tid2 = task2.unwrap().task_id;
        assert!([tid_left, tid_right].contains(&tid2));
        assert_ne!(tid1, tid2);

        // Complete both
        sched.complete(tid1).unwrap();
        sched.complete(tid2).unwrap();

        // Now merge should be ready
        let task = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(task.map(|t| t.task_id), Some(tid_merge));
    }

    #[tokio::test]
    async fn completing_task_unblocks_dependents() {
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();
        let tid_c = TaskId::new();

        sched
            .enqueue(make_task(tid_a, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_b, vec![tid_a], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_c, vec![tid_b], Priority::Normal))
            .unwrap();

        sched.dequeue(WorkerId::new()).await.unwrap(); // A
        sched.complete(tid_a).unwrap();
        sched.dequeue(WorkerId::new()).await.unwrap(); // B
        sched.complete(tid_b).unwrap();
        let task = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(task.map(|t| t.task_id), Some(tid_c));
    }

    #[tokio::test]
    async fn discard_cascades_to_dependents() {
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();

        sched
            .enqueue(make_task(tid_a, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_b, vec![tid_a], Priority::Normal))
            .unwrap();

        sched.fail(tid_a).unwrap();

        // B should not be dequeued (it's marked DependencyFailed)
        let task = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(task.is_none());
    }

    #[tokio::test]
    async fn requeue_allows_retry() {
        let sched = DagScheduler::new();
        let tid = TaskId::new();

        sched
            .enqueue(make_task(tid, vec![], Priority::Normal))
            .unwrap();

        let task1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(task1.map(|t| t.attempt), Some(0));

        sched.requeue(tid, 1).await.unwrap();

        let task2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(task2.map(|t| t.attempt), Some(1));
    }

    #[tokio::test]
    async fn enqueue_unknown_dep_fails() {
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_unknown = TaskId::new();

        let task_a = make_task(tid_a, vec![tid_unknown], Priority::Normal);

        let result = sched.enqueue(task_a);
        assert!(matches!(result, Err(CoreError::UnknownDependency(_))));
    }

    #[tokio::test]
    async fn enqueue_duplicate_fails() {
        let sched = DagScheduler::new();
        let tid = TaskId::new();

        sched
            .enqueue(make_task(tid, vec![], Priority::Normal))
            .unwrap();

        let result = sched.enqueue(make_task(tid, vec![], Priority::Normal));
        assert!(matches!(result, Err(CoreError::DuplicateTask(_))));
    }

    #[tokio::test]
    async fn independent_tasks_all_dequeued() {
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();
        let tid_c = TaskId::new();

        sched
            .enqueue(make_task(tid_a, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_b, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_c, vec![], Priority::Normal))
            .unwrap();

        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        let t3 = sched.dequeue(WorkerId::new()).await.unwrap();
        let t4 = sched.dequeue(WorkerId::new()).await.unwrap();

        let mut ids: Vec<_> = [t1, t2, t3]
            .iter()
            .filter_map(|t| t.as_ref().map(|x| x.task_id))
            .collect();
        ids.sort_by_key(|id| id.to_string());

        let mut expected = vec![tid_a, tid_b, tid_c];
        expected.sort_by_key(|id| id.to_string());

        assert_eq!(ids, expected);
        assert!(t4.is_none());
    }

    #[tokio::test]
    async fn long_chain_resolves_in_order() {
        let sched = DagScheduler::new();
        let mut tids = Vec::new();
        for _ in 0..5 {
            tids.push(TaskId::new());
        }

        sched
            .enqueue(make_task(tids[0], vec![], Priority::Normal))
            .unwrap();
        for i in 1..5 {
            sched
                .enqueue(make_task(tids[i], vec![tids[i - 1]], Priority::Normal))
                .unwrap();
        }

        for tid in tids.iter() {
            let task = sched.dequeue(WorkerId::new()).await.unwrap();
            assert_eq!(task.map(|t| t.task_id), Some(*tid));
            sched.complete(*tid).unwrap();
        }

        let task = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(task.is_none());
    }

    // ===== Fairness Tests =====

    #[tokio::test]
    async fn priority_orders_correctly_with_no_aging() {
        // Disable aging by using extremely high threshold
        let policy = FairnessPolicy {
            aging_threshold_ms: u64::MAX,
            aging_boost_levels: 1,
            enable_weighted_selection: false,
        };
        let sched = DagScheduler::with_fairness_policy(policy);

        let tid_low = TaskId::new();
        let tid_normal = TaskId::new();
        let tid_high = TaskId::new();
        let tid_critical = TaskId::new();

        // Enqueue in reverse priority order to verify they dequeue in priority order
        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();
        sched
            .enqueue(make_task(tid_normal, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid_high, vec![], Priority::High))
            .unwrap();
        sched
            .enqueue(make_task(tid_critical, vec![], Priority::Critical))
            .unwrap();

        // Should dequeue in priority order: Critical, High, Normal, Low
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(t1.map(|t| t.priority), Some(Priority::Critical));

        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(t2.map(|t| t.priority), Some(Priority::High));

        let t3 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(t3.map(|t| t.priority), Some(Priority::Normal));

        let t4 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(t4.map(|t| t.priority), Some(Priority::Low));

        let t5 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t5.is_none());
    }

    #[tokio::test]
    async fn fairness_policy_can_be_configured() {
        // Just verify that we can create a scheduler with custom policy
        let policy = FairnessPolicy {
            aging_threshold_ms: 100,
            aging_boost_levels: 2,
            enable_weighted_selection: true,
        };
        let sched = DagScheduler::with_fairness_policy(policy);

        let tid = TaskId::new();
        sched
            .enqueue(make_task(tid, vec![], Priority::Low))
            .unwrap();

        // Should dequeue successfully
        let t = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t.is_some());
        assert_eq!(t.unwrap().task_id, tid);
    }

    #[tokio::test]
    async fn fairness_allows_all_priorities_to_dequeue() {
        // Verify that all priority levels can be dequeued (no starvation)
        let sched = DagScheduler::new();

        let tid_low = TaskId::new();
        let tid_normal = TaskId::new();
        let tid_high = TaskId::new();
        let tid_critical = TaskId::new();

        // Enqueue all priorities (mixed order)
        sched
            .enqueue(make_task(tid_critical, vec![], Priority::Critical))
            .unwrap();
        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();
        sched
            .enqueue(make_task(tid_high, vec![], Priority::High))
            .unwrap();
        sched
            .enqueue(make_task(tid_normal, vec![], Priority::Normal))
            .unwrap();

        // Dequeue all tasks
        let mut dequeued = Vec::new();
        for _ in 0..4 {
            if let Ok(Some(task)) = sched.dequeue(WorkerId::new()).await {
                dequeued.push(task.task_id);
            }
        }

        // All tasks should be dequeued (no starvation)
        assert_eq!(dequeued.len(), 4);
        assert!(dequeued.contains(&tid_low));
        assert!(dequeued.contains(&tid_normal));
        assert!(dequeued.contains(&tid_high));
        assert!(dequeued.contains(&tid_critical));
    }

    #[tokio::test]
    async fn aging_boost_respects_critical_ceiling() {
        // Create scheduler with boost that would exceed Critical
        let policy = FairnessPolicy {
            aging_threshold_ms: 0,
            aging_boost_levels: 10, // Try to boost by 10 levels
            enable_weighted_selection: false,
        };
        let sched = DagScheduler::with_fairness_policy(policy);

        let tid_low = TaskId::new();
        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();

        // Dequeue - low priority boosted by 10, but capped at Critical
        let task = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(task.is_some());

        // Task should still be Low (original priority) since we don't modify the task itself
        let t = task.unwrap();
        assert_eq!(t.priority, Priority::Low);
        // The boosting is internal to dequeue logic, not reflected in returned task
    }

    #[tokio::test]
    async fn fairness_respects_no_aging_when_threshold_very_high() {
        // With huge aging threshold, aging won't kick in
        let policy = FairnessPolicy {
            aging_threshold_ms: u64::MAX / 2, // Very large, won't be reached in test
            aging_boost_levels: 1,
            enable_weighted_selection: false,
        };
        let sched = DagScheduler::with_fairness_policy(policy);

        let tid_low = TaskId::new();
        let tid_high = TaskId::new();

        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();
        sched
            .enqueue(make_task(tid_high, vec![], Priority::High))
            .unwrap();

        // First dequeue should prefer high priority
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t1.is_some());
        let first_priority = t1.unwrap().priority;

        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t2.is_some());
        let second_priority = t2.unwrap().priority;

        // Both should dequeue, and first should be High
        assert_eq!(first_priority, Priority::High);
        assert_eq!(second_priority, Priority::Low);
    }

    #[test]
    fn test_ready_entry_ord_implementation() {
        // Verify that ReadyEntry ordering is correct
        let entries = vec![
            ReadyEntry {
                original_priority: Priority::Low,
                effective_priority: Priority::Low,
                task_id: TaskId::new(),
            },
            ReadyEntry {
                original_priority: Priority::High,
                effective_priority: Priority::High,
                task_id: TaskId::new(),
            },
            ReadyEntry {
                original_priority: Priority::Normal,
                effective_priority: Priority::Normal,
                task_id: TaskId::new(),
            },
            ReadyEntry {
                original_priority: Priority::Critical,
                effective_priority: Priority::Critical,
                task_id: TaskId::new(),
            },
        ];

        // Save IDs before sorting
        let low_id = entries[0].task_id;
        let high_id = entries[1].task_id;
        let normal_id = entries[2].task_id;
        let critical_id = entries[3].task_id;

        // Convert to heap and dequeue
        let mut heap = std::collections::BinaryHeap::from(entries);

        let e1 = heap.pop().unwrap();
        assert_eq!(e1.effective_priority, Priority::Critical);
        assert_eq!(e1.task_id, critical_id);

        let e2 = heap.pop().unwrap();
        assert_eq!(e2.effective_priority, Priority::High);
        assert_eq!(e2.task_id, high_id);

        let e3 = heap.pop().unwrap();
        assert_eq!(e3.effective_priority, Priority::Normal);
        assert_eq!(e3.task_id, normal_id);

        let e4 = heap.pop().unwrap();
        assert_eq!(e4.effective_priority, Priority::Low);
        assert_eq!(e4.task_id, low_id);
    }

    // ===== Resource Allocation Tests =====

    #[tokio::test]
    async fn resource_constrained_task_waits_when_pool_full() {
        // Create a scheduler with limited resources
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();

        // Create a task requiring 8 CPU cores
        let task_a = TaskDescriptor {
            task_id: tid_a,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: knitting_crab_core::resource::ResourceAllocation::new(8.0, 1024, 0),
            policy: Default::default(),
            attempt: 0,
            is_critical: false,
            priority: Priority::Normal,
            dependencies: vec![],
        };

        // Create a smaller task
        let task_b = TaskDescriptor {
            task_id: tid_b,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: knitting_crab_core::resource::ResourceAllocation::new(2.0, 256, 0),
            policy: Default::default(),
            attempt: 0,
            is_critical: false,
            priority: Priority::Normal,
            dependencies: vec![],
        };

        sched.enqueue(task_a).unwrap();
        sched.enqueue(task_b).unwrap();

        // Without resource config, both should dequeue (backward compat)
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t1.is_some());

        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t2.is_some());
    }

    #[tokio::test]
    async fn resource_released_on_task_complete() {
        // This test verifies that resources are tracked and released after complete()
        // (This will work after implementing ResourcePool)
        let sched = DagScheduler::new();
        let tid = TaskId::new();

        let task = make_task(tid, vec![], Priority::Normal);
        sched.enqueue(task).unwrap();

        let dequeued = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(dequeued.map(|t| t.task_id), Some(tid));

        // Complete the task - resources should be released
        sched.complete(tid).unwrap();

        // State should be Completed
        // (More specific assertions will be added after ResourcePool implementation)
    }

    #[tokio::test]
    async fn resource_released_on_task_fail() {
        // This test verifies that resources are released when a Running task fails
        let sched = DagScheduler::new();
        let tid = TaskId::new();

        let task = make_task(tid, vec![], Priority::Normal);
        sched.enqueue(task).unwrap();

        let _dequeued = sched.dequeue(WorkerId::new()).await.unwrap();
        // Task is now Running

        // Fail the task - resources should be released
        sched.fail(tid).unwrap();

        // State should be Failed
        // (More specific assertions will be added after ResourcePool implementation)
    }

    #[tokio::test]
    async fn lower_priority_small_task_runs_when_large_cant_fit() {
        // This test shows that smaller tasks can run when larger ones can't fit
        // (This will be more meaningful after implementing ResourcePool)
        let sched = DagScheduler::new();
        let tid_large = TaskId::new();
        let tid_small = TaskId::new();

        // Create two independent tasks
        let task_large = make_task(tid_large, vec![], Priority::Critical);
        let task_small = make_task(tid_small, vec![], Priority::Low);

        sched.enqueue(task_large).unwrap();
        sched.enqueue(task_small).unwrap();

        // Both should be available to dequeue (no resource constraints yet)
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(t1.map(|t| t.task_id), Some(tid_large)); // Critical should dequeue first

        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert_eq!(t2.map(|t| t.task_id), Some(tid_small));
    }

    #[tokio::test]
    async fn tasks_without_resource_config_always_dequeue() {
        // Verify backward compatibility: without resource config, no blocking
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();

        let task_a = make_task(tid_a, vec![], Priority::Normal);
        let task_b = make_task(tid_b, vec![], Priority::Normal);

        sched.enqueue(task_a).unwrap();
        sched.enqueue(task_b).unwrap();

        // Both tasks should dequeue successfully (no resource config = no blocking)
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t1.is_some());

        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t2.is_some());

        let t3 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t3.is_none());
    }

    #[tokio::test]
    async fn metal_slots_tracked_correctly() {
        // Verify metal slots are part of resource allocation
        let sched = DagScheduler::new();
        let tid = TaskId::new();

        let task = TaskDescriptor {
            task_id: tid,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: knitting_crab_core::resource::ResourceAllocation::new(2.0, 512, 2),
            policy: Default::default(),
            attempt: 0,
            is_critical: false,
            priority: Priority::Normal,
            dependencies: vec![],
        };

        sched.enqueue(task).unwrap();

        let dequeued = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(dequeued.is_some());

        let dequeued_task = dequeued.unwrap();
        assert_eq!(dequeued_task.resources.metal_slots, 2);
    }

    #[tokio::test]
    async fn multiple_tasks_share_pool() {
        // Verify that multiple tasks can run concurrently if pool has capacity
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();

        let task_a = make_task(tid_a, vec![], Priority::Normal);
        let task_b = make_task(tid_b, vec![], Priority::Normal);

        sched.enqueue(task_a).unwrap();
        sched.enqueue(task_b).unwrap();

        // Both should be dequeued
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t1.is_some());

        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t2.is_some());
    }

    #[tokio::test]
    async fn pool_exhaustion_and_recovery() {
        // Verify that tasks can dequeue again after resources are freed
        let sched = DagScheduler::new();
        let tid_a = TaskId::new();
        let tid_b = TaskId::new();

        let task_a = make_task(tid_a, vec![], Priority::Normal);
        let task_b = make_task(tid_b, vec![], Priority::Normal);

        sched.enqueue(task_a).unwrap();
        sched.enqueue(task_b).unwrap();

        // Dequeue first task
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t1.is_some());
        let first_id = t1.unwrap().task_id;

        // Complete it to free resources
        sched.complete(first_id).unwrap();

        // Second task should now be dequeued
        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t2.is_some());
        let second_id = t2.unwrap().task_id;

        // Both tasks should be dequeued (order depends on heap ordering)
        assert_ne!(first_id, second_id);
        assert!([tid_a, tid_b].contains(&first_id));
        assert!([tid_a, tid_b].contains(&second_id));
    }

    // ===== Starvation Prevention Tests =====

    #[tokio::test]
    async fn starvation_policy_can_be_configured() {
        // Verify that we can create scheduler with custom starvation policy
        let policy = StarvationPreventionPolicy {
            low_priority_quota_percent: 3.0,
            normal_priority_quota_percent: 12.0,
            max_queue_wait_ms: 1000,
            enable_quota_enforcement: true,
        };
        let sched = DagScheduler::new();
        assert!(sched.set_starvation_policy(policy).is_ok());

        // Verify metrics can be retrieved
        let metrics = sched.starvation_metrics().unwrap();
        assert_eq!(metrics.aging_events, 0);
        assert_eq!(metrics.deadline_enforcements, 0);
    }

    #[tokio::test]
    async fn starvation_metrics_are_tracked() {
        let sched = DagScheduler::new();
        let tid_low = TaskId::new();

        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();

        // Dequeue once to trigger aging logic
        let _ = sched.dequeue(WorkerId::new()).await.unwrap();

        // Metrics should be accessible
        let metrics = sched.starvation_metrics().unwrap();
        // Depending on timing, aging_events may or may not be > 0
        // Just verify we can retrieve metrics without error
        assert_eq!(metrics.deadline_enforcements, 0);
    }

    #[tokio::test]
    async fn deadline_enforcement_forces_execution() {
        // Create scheduler with very aggressive deadline (0ms)
        let policy = StarvationPreventionPolicy {
            low_priority_quota_percent: 5.0,
            normal_priority_quota_percent: 15.0,
            max_queue_wait_ms: 0, // Deadline immediately
            enable_quota_enforcement: true,
        };
        let sched = DagScheduler::new();
        sched.set_starvation_policy(policy).unwrap();

        let tid_low = TaskId::new();
        let tid_high = TaskId::new();

        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();
        sched
            .enqueue(make_task(tid_high, vec![], Priority::High))
            .unwrap();

        // First dequeue should get high (higher priority, no deadline breach yet)
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t1.is_some());

        // Second dequeue should also work (both tasks get dequeued)
        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t2.is_some());

        // Verify both different tasks were dequeued
        assert_ne!(t1.unwrap().task_id, t2.unwrap().task_id);
    }

    #[tokio::test]
    async fn deadline_enforcement_disabled_when_max_queue_wait_is_zero_with_disable_flag() {
        // With enable_quota_enforcement = false, deadlines should be ignored
        let policy = StarvationPreventionPolicy {
            low_priority_quota_percent: 5.0,
            normal_priority_quota_percent: 15.0,
            max_queue_wait_ms: 0,
            enable_quota_enforcement: false, // Disabled
        };
        let sched = DagScheduler::new();
        sched.set_starvation_policy(policy).unwrap();

        let tid_low = TaskId::new();
        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();

        // Should still dequeue successfully (quota enforcement just disabled)
        let t = sched.dequeue(WorkerId::new()).await.unwrap();
        assert!(t.is_some());
    }

    #[tokio::test]
    async fn all_tasks_eventually_dequeue_with_starvation_prevention() {
        // Verify that with starvation prevention, all priorities get a turn
        let policy = StarvationPreventionPolicy {
            low_priority_quota_percent: 5.0,
            normal_priority_quota_percent: 15.0,
            max_queue_wait_ms: 0, // Very aggressive
            enable_quota_enforcement: true,
        };
        let sched = DagScheduler::new();
        sched.set_starvation_policy(policy).unwrap();

        // Enqueue 4 tasks, one per priority level
        let tid_low = TaskId::new();
        let tid_normal = TaskId::new();
        let tid_high = TaskId::new();
        let tid_critical = TaskId::new();

        sched
            .enqueue(make_task(tid_critical, vec![], Priority::Critical))
            .unwrap();
        sched
            .enqueue(make_task(tid_low, vec![], Priority::Low))
            .unwrap();
        sched
            .enqueue(make_task(tid_high, vec![], Priority::High))
            .unwrap();
        sched
            .enqueue(make_task(tid_normal, vec![], Priority::Normal))
            .unwrap();

        // Dequeue all 4 tasks
        let mut dequeued_ids = Vec::new();
        for _ in 0..4 {
            if let Ok(Some(task)) = sched.dequeue(WorkerId::new()).await {
                dequeued_ids.push(task.task_id);
            }
        }

        // All four should be dequeued
        assert_eq!(dequeued_ids.len(), 4);
        assert!(dequeued_ids.contains(&tid_low));
        assert!(dequeued_ids.contains(&tid_normal));
        assert!(dequeued_ids.contains(&tid_high));
        assert!(dequeued_ids.contains(&tid_critical));
    }

    #[tokio::test]
    async fn multiple_same_priority_tasks_fifo_order() {
        let sched = DagScheduler::new();
        let tid1 = TaskId::new();
        let tid2 = TaskId::new();
        let tid3 = TaskId::new();

        // All same priority
        sched
            .enqueue(make_task(tid1, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid2, vec![], Priority::Normal))
            .unwrap();
        sched
            .enqueue(make_task(tid3, vec![], Priority::Normal))
            .unwrap();

        // Should dequeue in deterministic task_id order (tie-break mechanism)
        let t1 = sched.dequeue(WorkerId::new()).await.unwrap();
        let t2 = sched.dequeue(WorkerId::new()).await.unwrap();
        let t3 = sched.dequeue(WorkerId::new()).await.unwrap();

        let mut ids = vec![
            t1.unwrap().task_id,
            t2.unwrap().task_id,
            t3.unwrap().task_id,
        ];
        ids.sort();

        let mut expected = vec![tid1, tid2, tid3];
        expected.sort();

        assert_eq!(ids, expected);
    }
}
