use thiserror::Error;

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

    #[error("database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}
