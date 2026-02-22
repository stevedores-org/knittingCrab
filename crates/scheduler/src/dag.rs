//! DAG-based task scheduler enforcing prerequisite ordering and cycle detection.
//!
//! The `DagScheduler` implements a directed acyclic graph (DAG) scheduler that:
//! - Rejects cyclic task dependencies at enqueue time
//! - Ensures tasks do not run before their dependencies complete
//! - Maintains ready-to-execute tasks in a priority-ordered heap
//! - Cascades failure across dependent tasks
//!
//! Hard invariants:
//! 1. No task runs before its dependencies complete
//! 2. Cycles are rejected at enqueue time with a clear error
//! 3. All changes flow through the `Queue` trait — `WorkerRuntime` is unchanged

use async_trait::async_trait;
use std::cmp::Ordering as CmpOrdering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;

use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::priority::Priority;
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

/// Entry in the ready-to-execute heap, ordered by priority (desc) then task_id (asc).
#[derive(Debug, Clone, Eq, PartialEq)]
struct ReadyEntry {
    priority: Priority,
    task_id: TaskId,
}

impl Ord for ReadyEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Max-heap by priority (desc)
        match other.priority.cmp(&self.priority) {
            CmpOrdering::Equal => {
                // Tie-break by task_id (asc) for determinism
                self.task_id.cmp(&other.task_id)
            }
            other => other,
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
    /// Max-heap of tasks ready to run (ordered by priority desc, then task_id asc).
    ready: std::collections::BinaryHeap<ReadyEntry>,
}

impl Inner {
    fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            states: HashMap::new(),
            successors: HashMap::new(),
            in_degree: HashMap::new(),
            ready: std::collections::BinaryHeap::new(),
        }
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

/// DAG-based scheduler enforcing prerequisite ordering and cycle detection.
pub struct DagScheduler {
    inner: Arc<Mutex<Inner>>,
}

impl DagScheduler {
    /// Create a new DAG scheduler.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new())),
        }
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

        // Set initial state
        let initial_state = if in_degree == 0 {
            TaskState::Ready
        } else {
            TaskState::Pending
        };
        inner.states.insert(task_id, initial_state);
        inner.in_degree.insert(task_id, in_degree);

        // If ready, add to heap
        if initial_state == TaskState::Ready {
            inner.ready.push(ReadyEntry {
                priority: task.priority,
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

        // Unblock successors (clone to avoid borrow issues)
        let successors = inner.successors.get(&task_id).cloned().unwrap_or_default();
        for succ_id in successors {
            if let Some(degree) = inner.in_degree.get_mut(&succ_id) {
                *degree -= 1;
                if *degree == 0 {
                    if let Some(TaskState::Pending) = inner.states.get(&succ_id) {
                        inner.states.insert(succ_id, TaskState::Ready);
                        // Get priority before the mutable borrow
                        let priority = inner.tasks.get(&succ_id).map(|t| t.priority);
                        if let Some(priority) = priority {
                            inner.ready.push(ReadyEntry {
                                priority,
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

        while let Some(entry) = inner.ready.pop() {
            // Verify task is still in Ready state (might be stale from heap).
            // Stale entries occur when tasks are failed/discarded while in the ready heap.
            // We filter them out here rather than removing them immediately for simplicity,
            // accepting the trade-off of temporary heap bloat during high-failure scenarios.
            if let Some(TaskState::Ready) = inner.states.get(&entry.task_id) {
                if let Some(task) = inner.tasks.get(&entry.task_id).cloned() {
                    inner.states.insert(entry.task_id, TaskState::Running);
                    return Ok(Some(task));
                }
            }
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
                inner.ready.push(ReadyEntry { priority, task_id });
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
}
