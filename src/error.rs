use thiserror::Error;

/// All errors that can arise within the knitting-crab scheduler.
#[derive(Debug, Error)]
pub enum SchedulerError {
    // ── System-level failures ──────────────────────────────────────────────
    /// A task requested more resources than the system can provide.
    #[error("Resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),

    /// The lease for the given task has passed its expiry time.
    #[error("Lease expired for task {task_id}")]
    LeaseExpired { task_id: String },

    /// A worker failed to send a heartbeat within the required interval.
    #[error("Heartbeat missed for worker {worker_id}")]
    HeartbeatMissed { worker_id: String },

    /// The task dependency graph contains a deadlock cycle at runtime.
    #[error("Deadlock detected in task graph")]
    DeadlockDetected,

    /// A static cycle was detected in the submitted DAG.
    #[error("Cycle detected in task DAG")]
    CycleDetected,

    /// The scheduler could not spawn a worker subprocess.
    #[error("Worker spawn failed: {0}")]
    WorkerSpawnFailed(String),

    /// An error occurred in the artifact cache layer.
    #[error("Cache error: {0}")]
    CacheError(String),

    /// A Mutex for the named resource could not be acquired.
    #[error("Lock contention on resource {resource}")]
    LockContention { resource: String },

    // ── Task-level failures ────────────────────────────────────────────────
    /// The task process exited with a non-zero code.
    #[error("Task {task_id} failed with exit code {exit_code}")]
    TaskFailed { task_id: String, exit_code: i32 },

    /// The task exceeded its configured wall-clock timeout.
    #[error("Task {task_id} timed out after {secs}s")]
    TaskTimeout { task_id: String, secs: u64 },

    /// A mandatory test gate failed for the given task.
    #[error("Test gate failed for task {task_id}")]
    TestGateFailed { task_id: String },

    /// An agent exceeded its allowed tool-call or wall-clock budget.
    #[error(
        "Budget exceeded for agent {agent_id}: tool calls={tool_calls}, wall_secs={wall_secs}"
    )]
    BudgetExceeded {
        agent_id: String,
        tool_calls: u32,
        wall_secs: u64,
    },

    /// Another agent already holds the exclusive lock on this goal.
    #[error("Goal already locked: repo={repo}, branch={branch}")]
    GoalAlreadyLocked { repo: String, branch: String },

    /// The task has been retried the maximum number of times.
    #[error("Max retries exceeded for task {task_id}")]
    MaxRetriesExceeded { task_id: String },

    /// A task ID referenced in a dependency list does not exist in the plan.
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    /// Transparent wrapper around [`std::io::Error`].
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, SchedulerError>;
