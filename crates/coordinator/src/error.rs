use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoordinatorError {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Core error: {0}")]
    Core(String),

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type CoordinatorResult<T> = Result<T, CoordinatorError>;
