use thiserror::Error;

use crate::ids::TaskId;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("task already has an active lease")]
    AlreadyLeased,

    #[error("lease not found")]
    LeaseNotFound,

    #[error("stale lease id")]
    StaleLeaseId,

    #[error("invalid state transition: {0}")]
    InvalidStateTransition(String),

    #[error("resource allocation failed: {0}")]
    ResourceAllocationFailed(String),

    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("lock poisoned: {0}")]
    LockPoisoned(String),

    #[error("database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("cyclic dependency detected involving task {0}")]
    CyclicDependency(TaskId),

    #[error("unknown dependency: task {0} not in graph")]
    UnknownDependency(TaskId),

    #[error("duplicate task id: {0}")]
    DuplicateTask(TaskId),

    #[error("dependency failed for task {0}")]
    DependencyFailed(TaskId),

    #[error("goal lock conflict: another task is already working on goal '{goal}'")]
    GoalLockConflict { goal: String },

    #[error("budget exceeded: {reason}")]
    BudgetExceeded { reason: String },
}
