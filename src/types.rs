use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

// ── Priority ──────────────────────────────────────────────────────────────────

/// Scheduling priority tier.
///
/// Lower numeric values correspond to higher urgency.  The `Ord` implementation
/// reflects that: `Critical < High < Normal < Low` (in terms of the raw
/// discriminant), and we derive standard ordering so that `Critical` compares
/// *less than* `Low`, which makes min-heap usage natural.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Priority {
    /// Highest urgency – preempts everything else.
    Critical = 0,
    /// High urgency.
    High = 1,
    /// Default priority for most tasks.
    Normal = 2,
    /// Background / best-effort work.
    Low = 3,
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// `Critical` sorts *before* `Low`, matching the discriminant ordering.
impl Ord for Priority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

// ── TaskStatus ────────────────────────────────────────────────────────────────

/// Lifecycle state of a [`WorkItem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    /// Queued, waiting for resources or dependencies.
    Pending,
    /// Currently executing on a worker.
    Running {
        /// ID of the [`crate::worker::AgentWorker`] that owns this task.
        worker_id: String,
        /// ID of the active [`crate::lease::Lease`].
        lease_id: String,
    },
    /// Finished successfully.
    Completed {
        /// The process exit code (should be `0`).
        exit_code: i32,
    },
    /// Finished unsuccessfully.
    Failed {
        /// The non-zero exit code returned by the process.
        exit_code: i32,
        /// Number of times this task has already been retried.
        retries: u32,
    },
    /// Explicitly cancelled by the scheduler or an operator.
    Cancelled,
}

// ── ResourceBudget ────────────────────────────────────────────────────────────

/// Requested compute resources for a single task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceBudget {
    /// Number of CPU cores required (fractional values allowed).
    pub cpu_cores: f32,
    /// RAM required in mebibytes.
    pub ram_mb: u64,
    /// Apple Silicon GPU/Metal compute slots required.
    pub metal_slots: u32,
}

impl ResourceBudget {
    /// Constructs a new budget with the given resource amounts.
    pub fn new(cpu_cores: f32, ram_mb: u64, metal_slots: u32) -> Self {
        Self {
            cpu_cores,
            ram_mb,
            metal_slots,
        }
    }
}

// ── WorkItem ──────────────────────────────────────────────────────────────────

/// A unit of work submitted to the scheduler.
#[derive(Debug, Clone)]
pub struct WorkItem {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Human-readable task name.
    pub name: String,
    /// Git repository the task operates on.
    pub repo: String,
    /// Branch within `repo`.
    pub branch: String,
    /// Command and arguments to execute (argv style).
    pub command: Vec<String>,
    /// Extra environment variables injected into the subprocess.
    pub env: HashMap<String, String>,
    /// Scheduling priority tier.
    pub priority: Priority,
    /// Resource envelope the task may consume.
    pub resource_budget: ResourceBudget,
    /// Maximum number of automatic retries on failure.
    pub max_retries: u32,
    /// Hard wall-clock timeout in seconds.
    pub timeout_secs: u64,
    /// IDs of tasks that must complete before this one can start.
    pub dependencies: Vec<String>,
    /// Optional content-addressable hash of the high-level goal.
    pub goal_hash: Option<String>,
    /// Moment the item was first created (used for aging).
    pub created_at: Instant,
    /// Extra priority points accumulated via the aging policy; starts at `0`.
    pub priority_age_bonus: u32,
}

/// Builder for constructing a [`WorkItem`].
///
/// Required fields must be set before calling [`WorkItemBuilder::build`].
#[derive(Default)]
pub struct WorkItemBuilder {
    name: String,
    repo: String,
    branch: String,
    command: Vec<String>,
    env: HashMap<String, String>,
    priority: Option<Priority>,
    resource_budget: Option<ResourceBudget>,
    max_retries: u32,
    timeout_secs: u64,
    dependencies: Vec<String>,
    goal_hash: Option<String>,
}

impl WorkItemBuilder {
    /// Creates a new builder with required name, repo, branch, and command.
    pub fn new(
        name: impl Into<String>,
        repo: impl Into<String>,
        branch: impl Into<String>,
        command: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            repo: repo.into(),
            branch: branch.into(),
            command,
            ..Default::default()
        }
    }

    /// Sets extra environment variables.
    pub fn env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Sets the scheduling priority (default: [`Priority::Normal`]).
    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Sets the resource budget (default: 1 CPU core, 256 MiB RAM, 0 Metal slots).
    pub fn resource_budget(mut self, budget: ResourceBudget) -> Self {
        self.resource_budget = Some(budget);
        self
    }

    /// Sets the maximum number of automatic retries (default: `0`).
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Sets the hard wall-clock timeout in seconds (default: `300`).
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Sets the list of task IDs that must complete before this task starts.
    pub fn dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    /// Sets an optional goal hash for deduplication locking.
    pub fn goal_hash(mut self, hash: impl Into<String>) -> Self {
        self.goal_hash = Some(hash.into());
        self
    }

    /// Builds the [`WorkItem`].
    pub fn build(self) -> WorkItem {
        WorkItem {
            id: uuid::Uuid::new_v4().to_string(),
            name: self.name,
            repo: self.repo,
            branch: self.branch,
            command: self.command,
            env: self.env,
            priority: self.priority.unwrap_or(Priority::Normal),
            resource_budget: self
                .resource_budget
                .unwrap_or_else(|| ResourceBudget::new(1.0, 256, 0)),
            max_retries: self.max_retries,
            timeout_secs: if self.timeout_secs == 0 {
                300
            } else {
                self.timeout_secs
            },
            dependencies: self.dependencies,
            goal_hash: self.goal_hash,
            created_at: Instant::now(),
            priority_age_bonus: 0,
        }
    }
}

impl WorkItem {
    /// Computes the effective priority score used for queue ordering.
    ///
    /// A *lower* return value means *higher* urgency (matching `Priority`'s
    /// discriminant semantics).  The age bonus is subtracted so that tasks
    /// that have waited a long time gradually rise above newer tasks at the
    /// same nominal priority.
    pub fn effective_priority_score(&self) -> i64 {
        (self.priority as i64) * 100 - (self.priority_age_bonus as i64)
    }
}

impl PartialEq for WorkItem {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for WorkItem {}

impl PartialOrd for WorkItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Tasks with a lower effective priority score come *first* (highest urgency).
impl Ord for WorkItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.effective_priority_score()
            .cmp(&other.effective_priority_score())
    }
}

// ── WorkerEvent ───────────────────────────────────────────────────────────────

/// Events emitted by an [`crate::worker::AgentWorker`] during task execution.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// The worker process has been launched.
    Started {
        /// ID of the task being executed.
        task_id: String,
        /// ID of the worker that launched it.
        worker_id: String,
    },
    /// A single line of stdout/stderr output.
    LogLine {
        /// ID of the producing task.
        task_id: String,
        /// The log line content.
        line: String,
    },
    /// The worker process exited successfully or with a known code.
    Completed {
        /// ID of the finished task.
        task_id: String,
        /// Process exit code.
        exit_code: i32,
    },
    /// Periodic liveness signal from a running worker.
    HeartbeatTick {
        /// ID of the worker sending the heartbeat.
        worker_id: String,
    },
    /// The worker process terminated with an error.
    Failed {
        /// ID of the failed task.
        task_id: String,
        /// Human-readable error description.
        error: String,
    },
}

// ── AgentBudget ───────────────────────────────────────────────────────────────

/// Tracks consumed budget for a single agent session.
#[derive(Debug, Clone, Default)]
pub struct AgentBudget {
    /// Maximum number of tool calls permitted.
    pub max_tool_calls: u32,
    /// Maximum wall-clock seconds permitted.
    pub max_wall_secs: u64,
    /// Tool calls consumed so far.
    pub tool_calls_used: u32,
    /// Wall-clock seconds consumed so far.
    pub wall_secs_used: u64,
}

impl AgentBudget {
    /// Creates a new budget with the given limits.
    pub fn new(max_tool_calls: u32, max_wall_secs: u64) -> Self {
        Self {
            max_tool_calls,
            max_wall_secs,
            tool_calls_used: 0,
            wall_secs_used: 0,
        }
    }

    /// Returns `true` if either budget dimension has been exceeded.
    pub fn is_exceeded(&self) -> bool {
        self.tool_calls_used >= self.max_tool_calls || self.wall_secs_used >= self.max_wall_secs
    }
}
